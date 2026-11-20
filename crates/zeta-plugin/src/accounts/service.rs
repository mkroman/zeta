//! The host-attached account service: session tracking and per-user preferences in one place.
//!
//! The host constructs one instance over the shared pool, attaches it to the context, feeds it
//! every incoming IRC message through [`Accounts::track`], and spawns
//! [`Accounts::run_flush`](Accounts::run_flush) to move observations to the database. Clones
//! share the same state, cache and pool, so a change written through the `.set` command is
//! visible to every reader immediately.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use irc::proto::{Command, Message};
use parking_lot::Mutex;
use tracing::{Instrument, warn};

use crate::accounts::hostmasks::ObservedHostmask;
use crate::accounts::identity::UserIdentity;
use crate::accounts::preferences::Preferences;
use crate::accounts::repository::AccountRepository;
use crate::database::{Database, DbError};
use crate::event::account;

/// How often the dirty sessions are flushed to the database.
const FLUSH_INTERVAL: Duration = Duration::from_mins(1);

/// The IRC sessions held in memory before the oldest is refused.
///
/// The map only feeds hostmask tracking; a session refused at the cap is re-created by the
/// next message from that nick.
const SESSION_CAPACITY: usize = 4096;

/// One IRC session the tracker currently maps to an account: the nick's lowercase form, the
/// account it is logged into, and the hostmask it connected from.
///
/// `dirty` marks a session whose account or hostmask changed since the last flush, so a flush
/// writes only what changed. A session with an empty hostmask was learned from an `ACCOUNT`
/// notification alone and cannot be recorded until a message from the nick reveals its prefix.
#[derive(Clone, Eq, PartialEq)]
struct Session {
    account: String,
    hostmask: String,
    dirty: bool,
}

/// The account subsystem's service: session tracking and preferences over the shared pool.
#[derive(Clone)]
pub struct Accounts {
    /// The per-user preferences facet.
    preferences: Arc<Preferences>,
    /// The repository the tracking observations go through.
    repository: Arc<AccountRepository>,
    /// The tracked IRC sessions, keyed by lowercase nick.
    sessions: Arc<Mutex<HashMap<String, Session>>>,
}

// Manual implementation: the pool, the caches and the session state are opaque internals.
impl fmt::Debug for Accounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Accounts").finish_non_exhaustive()
    }
}

impl Accounts {
    /// Constructs the service over the shared database pool.
    ///
    /// The host calls this once at startup, attaches it with
    /// [`Context::with_accounts`](crate::Context::with_accounts), and spawns
    /// [`Accounts::run_flush`](Self::run_flush) beside its other tasks.
    #[must_use]
    pub fn new(db: Database) -> Self {
        let repository = Arc::new(AccountRepository::new(db));

        Self {
            preferences: Arc::new(Preferences::new(Arc::clone(&repository))),
            repository,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The per-user preferences facet.
    ///
    /// Plugins that resolve preferences per command clone the handle once at construction —
    /// cloning shares the cache and the pool.
    #[must_use]
    pub const fn preferences(&self) -> &Arc<Preferences> {
        &self.preferences
    }

    /// Feeds one incoming IRC message to the tracker.
    ///
    /// The sessions live in memory only — nothing here touches the database, so tracking is as
    /// cheap as the messages are frequent. The database learns of a session when
    /// [`Accounts::run_flush`](Self::run_flush) moves the sessions marked dirty since the last
    /// flush in one batched upsert.
    ///
    /// The notifications the IRCv3 `account-notify`, `extended-join` and `account-tag`
    /// capabilities provide are the inputs: a message from a logged-in sender carries the
    /// sender's account as a tag and its hostmask as the prefix, `ACCOUNT` announces account
    /// changes for nicks in shared channels (including logouts, as `*`), and the
    /// `extended-join` form of `JOIN` pairs the joining hostmask with its account. `NICK`
    /// renames a session, `QUIT` drops it.
    pub fn track(&self, message: &Message) {
        let Some(irc::proto::Prefix::Nickname(nick, username, hostname)) = &message.prefix else {
            // Server-originated messages carry no session to track; `ACCOUNT` is handled below.
            return self.track_notification(message);
        };

        let hostmask = format!("{nick}!{username}@{hostname}");
        let nick = nick.to_ascii_lowercase();

        match &message.command {
            // The extended-join form of `JOIN` pairs the joining hostmask with its account;
            // the parser hands the account to us in the channel-key position.
            Command::JOIN(_, Some(acct), _) if acct != "*" => {
                self.observe_session(&nick, acct, &hostmask);
            }
            Command::NICK(new_nick) => self.rename_session(&nick, new_nick),
            Command::QUIT(_) => {
                self.sessions.lock().remove(&nick);
            }
            _ => {
                // A message from a logged-in sender carries its account as a tag; senders who
                // have not identified carry none, or `*`, and are not tracked at all.
                if let Some(acct) = account(message) {
                    self.observe_session(&nick, acct, &hostmask);
                }
            }
        }
    }

    /// Handles the server-originated notifications: `ACCOUNT nick <account|*>` from
    /// `account-notify`.
    ///
    /// An `ACCOUNT` for a nick the tracker knows updates its account; one for a nick it does
    /// not know creates the session without a hostmask, which the next message from that nick
    /// fills in. `*` drops the session: the user logged out, and their messages stop carrying
    /// the account tag.
    fn track_notification(&self, message: &Message) {
        let Command::Raw(command, params) = &message.command else {
            return;
        };

        if !command.eq_ignore_ascii_case("ACCOUNT") {
            return;
        }

        let Some((nick, account)) = params.first().zip(params.get(1)) else {
            return;
        };
        let nick = nick.to_ascii_lowercase();

        if account == "*" {
            self.sessions.lock().remove(&nick);

            return;
        }

        let mut sessions = self.sessions.lock();

        if let Some(entry) = sessions.get_mut(&nick) {
            if entry.account != account.to_ascii_lowercase() {
                entry.account = account.to_ascii_lowercase();
                entry.dirty = true;
            }

            return;
        }

        // The cap holds here too, not only in `observe_session`: notifications for unknown
        // nicks must not grow the map past it either.
        if sessions.len() >= SESSION_CAPACITY {
            return;
        }

        sessions.insert(
            nick,
            Session {
                account: account.to_ascii_lowercase(),
                hostmask: String::new(),
                dirty: false,
            },
        );
        drop(sessions);
    }

    /// Inserts or updates the session for `nick`, marking it dirty when anything changed.
    ///
    /// The account is stored lowercased, like [`UserIdentity::key`] spells the identities the
    /// rows are stored under, so a flushed observation is always stored — and looked up — under
    /// the canonical form.
    fn observe_session(&self, nick: &str, account: &str, hostmask: &str) {
        let account = account.to_ascii_lowercase();
        let mut sessions = self.sessions.lock();

        if sessions.len() >= SESSION_CAPACITY && !sessions.contains_key(nick) {
            return;
        }

        let entry = sessions.entry(nick.to_owned()).or_insert_with(|| Session {
            account: account.clone(),
            hostmask: hostmask.to_owned(),
            dirty: true,
        });

        if entry.account != account || entry.hostmask != hostmask {
            account.clone_into(&mut entry.account);
            hostmask.clone_into(&mut entry.hostmask);
            entry.dirty = true;
        }

        drop(sessions);
    }

    /// Moves a session to its new nick, keeping its account, hostmask and dirty flag.
    fn rename_session(&self, old_nick: &str, new_nick: &str) {
        let mut sessions = self.sessions.lock();

        if let Some(session) = sessions.remove(old_nick) {
            sessions.insert(new_nick.to_ascii_lowercase(), session);
        }
    }

    /// Records that `identity` was seen from its hostmask.
    ///
    /// The session tracker records automatically; this records one observation directly, e.g.
    /// for an identity learned outside the IRC message flow.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the upsert fails.
    pub async fn record(&self, identity: &UserIdentity) -> Result<(), DbError> {
        self.repository
            .observe_hostmask(identity.key(), identity.hostmask())
            .await
    }

    /// The hostmasks `identity` has been seen from, most recently used first.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the query fails.
    pub async fn hostmasks(
        &self,
        identity: &UserIdentity,
    ) -> Result<Vec<ObservedHostmask>, DbError> {
        self.repository.hostmasks(identity.key()).await
    }

    /// Moves every session marked dirty since the last flush to the database in one batched
    /// upsert, returning how many observations were recorded.
    ///
    /// Sessions whose hostmask is still unknown (learned from an `ACCOUNT` notification alone)
    /// stay dirty until a message from the nick reveals the prefix.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the batched upsert fails; the sessions stay dirty, so the next
    /// flush retries them.
    pub async fn flush(&self) -> Result<usize, DbError> {
        // The snapshot borrows out of the map without clearing anything: the flag moves only
        // once the write has succeeded.
        let batch: Vec<(String, String, String)> = {
            let sessions = self.sessions.lock();

            sessions
                .iter()
                .filter(|(_, session)| session.dirty && !session.hostmask.is_empty())
                .map(|(nick, session)| {
                    (
                        nick.clone(),
                        session.account.clone(),
                        session.hostmask.clone(),
                    )
                })
                .collect()
        };

        if batch.is_empty() {
            return Ok(0);
        }

        let observations: Vec<(String, String)> = batch
            .iter()
            .map(|(_, account, hostmask)| (account.clone(), hostmask.clone()))
            .collect();
        self.repository.observe_hostmasks(&observations).await?;

        // Recorded. A session whose account or hostmask changed while the write ran was
        // re-marked dirty by the tracker, so only the ones still holding what was written are
        // cleared — the changed ones retry with their new values on the next flush.
        {
            let mut sessions = self.sessions.lock();

            for (nick, account, hostmask) in &batch {
                if let Some(session) = sessions.get_mut(nick)
                    && session.account == *account
                    && session.hostmask == *hostmask
                {
                    session.dirty = false;
                }
            }
        }

        Ok(batch.len())
    }

    /// Flushes the dirty sessions on the flush interval, until the task is dropped.
    ///
    /// The host spawns this once at startup. Every tick opens its own `accounts.flush` span —
    /// the tracker's flushes are background work, so without a per-unit span neither the
    /// failure log nor the batched upsert's database span would reach the backend. A failed
    /// flush is logged with its operator-grade chain, and the failed sessions stay dirty for
    /// the next tick.
    pub async fn run_flush(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            ticker.tick().await;

            async {
                match self.flush().await {
                    Ok(0) => {}
                    Ok(recorded) => {
                        tracing::debug!("flushed {recorded} observed hostmasks");
                    }
                    Err(error) => {
                        warn!(
                            exception.message = %crate::Chain(&error),
                            "could not flush the observed hostmasks"
                        );
                    }
                }
            }
            .instrument(tracing::info_span!("accounts.flush"))
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use irc::proto::message::Tag;

    use super::*;

    /// Whether any session would be flushed on the next tick.
    ///
    /// The dirty flags are internal, so a test that asserts on the retry contract reads them
    /// through this instead of guessing from a flush count.
    fn has_dirty_sessions(accounts: &Accounts) -> bool {
        accounts
            .sessions
            .lock()
            .values()
            .any(|session| session.dirty)
    }

    /// A `PRIVMSG` from `nick!user@host` carrying the given tags.
    fn message(tags: &[(&str, &str)]) -> Message {
        let mut message = Message::new(None, "PRIVMSG", vec!["#test", "hello"])
            .expect("the message should parse");
        message.prefix = Some(irc::proto::Prefix::Nickname(
            "nick".to_owned(),
            "user".to_owned(),
            "host".to_owned(),
        ));
        message.tags = Some(
            tags.iter()
                .map(|(key, value)| Tag(key.to_string(), Some(value.to_string())))
                .collect(),
        );

        message
    }

    /// A `nick!user@host` prefix on `message`.
    fn with_hostmask(mut message: Message, nick: &str, user: &str, host: &str) -> Message {
        message.prefix = Some(irc::proto::Prefix::Nickname(
            nick.to_owned(),
            user.to_owned(),
            host.to_owned(),
        ));

        message
    }

    /// A server-originated `ACCOUNT nick <account|*>` notification.
    fn account_notification(nick: &str, account: &str) -> Message {
        Message::new(None, "ACCOUNT", vec![nick, account]).expect("the message should parse")
    }

    /// The extended-join form of `JOIN`: the prefix carries the hostmask, the account follows
    /// the channel.
    fn extended_join(nick: &str, user: &str, host: &str, account: &str) -> Message {
        let message = Message::new(None, "JOIN", vec!["#test", account, "the real name"])
            .expect("the message should parse");

        with_hostmask(message, nick, user, host)
    }

    /// The identity the tracker should record for `account`, from the default hostmask.
    fn tracked_identity(account: &str) -> UserIdentity {
        UserIdentity::from_parts(account, crate::event::Sender::new("nick", "user", "host"))
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn flush_records_the_sessions_tracking_marked_dirty(db: sqlx::PgPool) {
        let accounts = Accounts::new(db);

        // Nothing tracked, nothing flushed.
        assert_eq!(accounts.flush().await.unwrap(), 0);

        accounts.track(&message(&[("account", "alice")]));
        assert_eq!(accounts.flush().await.unwrap(), 1);

        let observed = accounts
            .hostmasks(&tracked_identity("alice"))
            .await
            .expect("the observations load");
        assert_eq!(observed[0].hostmask, "nick!user@host");

        // The same session again is not dirty, so nothing new is flushed.
        accounts.track(&message(&[("account", "alice")]));
        assert_eq!(accounts.flush().await.unwrap(), 0);
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn a_changed_hostmask_marks_the_session_dirty_again(db: sqlx::PgPool) {
        let accounts = Accounts::new(db);

        accounts.track(&message(&[("account", "alice")]));
        assert_eq!(accounts.flush().await.unwrap(), 1);

        // The nick reappears from another host: the session's hostmask changed, and the flush
        // records the new hostmask alongside the first one.
        accounts.track(&with_hostmask(
            message(&[("account", "alice")]),
            "nick",
            "user",
            "other.host",
        ));
        assert_eq!(accounts.flush().await.unwrap(), 1);

        let observed = accounts
            .hostmasks(&tracked_identity("alice"))
            .await
            .expect("the observations load");
        assert_eq!(observed.len(), 2);
        assert_eq!(
            observed[0].hostmask, "nick!user@other.host",
            "most recent first"
        );

        // The same hostmask again is not a change, so nothing new is flushed.
        accounts.track(&with_hostmask(
            message(&[("account", "alice")]),
            "nick",
            "user",
            "other.host",
        ));
        assert_eq!(accounts.flush().await.unwrap(), 0);
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn account_notifications_update_and_drop_sessions(db: sqlx::PgPool) {
        let accounts = Accounts::new(db);

        // An ACCOUNT for an unknown nick creates the session without a hostmask, and a flush
        // skips it: there is nothing to record yet.
        accounts.track(&account_notification("other", "bob"));
        assert_eq!(accounts.flush().await.unwrap(), 0);

        // The next message from the nick fills in the hostmask, and the session flushes under
        // the notified account.
        accounts.track(&with_hostmask(
            message(&[("account", "bob")]),
            "other",
            "u",
            "h",
        ));
        assert_eq!(accounts.flush().await.unwrap(), 1);
        assert_eq!(
            accounts
                .hostmasks(&tracked_identity("bob"))
                .await
                .expect("loads")[0]
                .hostmask,
            "other!u@h"
        );

        // A logout drops the session, and its messages stop being tracked.
        accounts.track(&account_notification("other", "*"));
        accounts.track(&with_hostmask(message(&[]), "other", "u", "h"));
        assert_eq!(accounts.flush().await.unwrap(), 0);
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn nick_renames_keep_the_session_and_extended_join_seeds_new_ones(db: sqlx::PgPool) {
        let accounts = Accounts::new(db);

        accounts.track(&message(&[("account", "alice")]));

        // The rename moves the session to the new nick with its account, hostmask and dirty
        // flag intact — still dirty from the track above, so the flush records it.
        accounts.track(&with_hostmask(
            Message::new(None, "NICK", vec!["newnick"]).expect("the message should parse"),
            "nick",
            "user",
            "host",
        ));
        assert_eq!(accounts.flush().await.unwrap(), 1);

        accounts.track(&extended_join("joiner", "j", "join.host", "carol"));
        assert_eq!(accounts.flush().await.unwrap(), 1);
        assert_eq!(
            accounts
                .hostmasks(&tracked_identity("carol"))
                .await
                .expect("loads")[0]
                .hostmask,
            "joiner!j@join.host"
        );

        // The extended-join placeholder for an unidentified joiner is not tracked.
        accounts.track(&extended_join("anon", "a", "anon.host", "*"));
        assert_eq!(accounts.flush().await.unwrap(), 0);
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn a_failed_flush_keeps_the_sessions_dirty(db: sqlx::PgPool) {
        let accounts = Accounts::new(db.clone());

        accounts.track(&message(&[("account", "alice")]));
        assert!(has_dirty_sessions(&accounts));

        // Take the database away: the batched upsert fails, and the sessions stay dirty for
        // the next flush instead of losing the batch.
        db.close().await;
        assert!(accounts.flush().await.is_err());
        assert!(
            has_dirty_sessions(&accounts),
            "a failed flush must not clear what it did not record"
        );
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn mixed_case_accounts_are_tracked_under_the_lowercase_identity(db: sqlx::PgPool) {
        let accounts = Accounts::new(db);

        // Services report the account in its registered spelling; the identity keys — and so
        // the rows — are lowercase regardless.
        accounts.track(&message(&[("account", "Alice")]));
        assert_eq!(accounts.flush().await.unwrap(), 1);

        let observed = accounts
            .hostmasks(&tracked_identity("alice"))
            .await
            .expect("the observations load");
        assert_eq!(observed[0].hostmask, "nick!user@host");

        // An `ACCOUNT` notification for the same account updates the same session instead of
        // creating a second one under another spelling.
        accounts.track(&account_notification("nick", "ALICE"));
        assert!(
            !has_dirty_sessions(&accounts),
            "the lowercased account is unchanged, so nothing is dirty"
        );
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn the_flush_upserts_its_batch_into_the_database(db: sqlx::PgPool) {
        let accounts = Accounts::new(db);

        accounts.track(&message(&[("account", "alice")]));
        accounts.track(&with_hostmask(
            message(&[("account", "bob")]),
            "bob",
            "b",
            "bob.host",
        ));
        assert_eq!(
            accounts.flush().await.unwrap(),
            2,
            "one batch, two observations"
        );

        // Flushing is idempotent per change: a clean session writes nothing.
        assert_eq!(accounts.flush().await.unwrap(), 0);

        let alice = tracked_identity("alice");
        let observed = accounts.hostmasks(&alice).await.expect("loads");
        assert_eq!(observed[0].hostmask, "nick!user@host");
    }
}
