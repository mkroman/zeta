//! The identity of a message's sender.

use irc::proto::Message;

use crate::event::{Sender, account};

/// The prefix marking an account-backed identity key.
const ACCOUNT_PREFIX: &str = "account:";

/// The prefix marking a hostmask-fallback identity key.
const HOSTMASK_PREFIX: &str = "hostmask:";

/// The stable identity of a message's sender, for everything the bot tracks per user.
///
/// The identity prefers the IRCv3 `account` tag — the services account the sender is logged
/// into, which survives reconnects, nick changes and new hosts — and falls back to the sender's
/// lowercased `user@host` when the network does not report accounts. The two kinds are
/// distinguishable in the stored key, so an account and a host can never collide into one
/// identity.
///
/// Built by the generated `identity()` accessor on every event; constructed directly in tests
/// and from already-parsed parts through [`UserIdentity::from_parts`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserIdentity {
    /// The key the database rows are stored under.
    key: String,
    /// The sender's `nick!user@host` at the time of the message.
    hostmask: String,
    /// Whether the identity came from a services account.
    accounted: bool,
}

impl UserIdentity {
    /// Resolves the identity of `message`'s sender, if it has a nickname prefix.
    ///
    /// The `account` tag decides the kind; without it (or with a `*` placeholder, which some
    /// networks send for users who are not logged in) the identity falls back to the sender's
    /// `user@host`.
    #[must_use]
    pub fn from_message(message: &Message) -> Option<Self> {
        let sender = Sender::from_message(message)?;
        let account = account(message);

        Some(Self::from_parts(account, sender))
    }

    /// Builds the identity from an optional services account and the sender's prefix parts.
    ///
    /// The hostmask is tracked for either kind: accounts move across hosts, so which hosts used
    /// an account is exactly what the tracking answers.
    #[must_use]
    pub fn from_parts(account: Option<&str>, sender: Sender<'_>) -> Self {
        let hostmask = format!("{}!{}@{}", sender.nick, sender.username, sender.hostname);

        match account
            .map(str::trim)
            .filter(|account| !account.is_empty() && *account != "*")
        {
            Some(account) => Self {
                key: format!("{ACCOUNT_PREFIX}{}", account.to_ascii_lowercase()),
                hostmask,
                accounted: true,
            },
            None => Self {
                key: format!(
                    "{HOSTMASK_PREFIX}{}@{}",
                    sender.username.to_ascii_lowercase(),
                    sender.hostname.to_ascii_lowercase(),
                ),
                hostmask,
                accounted: false,
            },
        }
    }

    /// The key the database rows are stored under.
    ///
    /// Prefixed with the identity kind, so an account can never collide with a hostmask
    /// fallback spelling the same string.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The sender's `nick!user@host` at the time of the message.
    ///
    /// Recorded for every identity, so the hosts an account-backed identity has been used from
    /// stay auditable even though the account itself moves across hosts.
    #[must_use]
    pub fn hostmask(&self) -> &str {
        &self.hostmask
    }

    /// Whether the identity is a services account, rather than a hostmask fallback.
    ///
    /// An account-backed identity follows the user across hosts and nick changes; a fallback
    /// identity is only as stable as the host they connect from.
    #[must_use]
    pub const fn is_account(&self) -> bool {
        self.accounted
    }
}

impl std::fmt::Display for UserIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.key)
    }
}

#[cfg(test)]
mod tests {
    use irc::proto::message::Tag;

    use super::*;

    /// A message from `nick!user@host`, carrying the given tags.
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

    #[test]
    fn an_account_tag_identifies_the_sender_across_hosts() {
        let identity = UserIdentity::from_message(&message(&[("account", "alice")]))
            .expect("the sender is known");

        assert!(identity.is_account());
        assert_eq!(identity.key(), "account:alice");
        assert_eq!(identity.hostmask(), "nick!user@host");

        // Account names are case-insensitive, like the services that issue them.
        assert_eq!(
            UserIdentity::from_parts(Some("ALICE"), Sender::new("n", "u", "h")).key(),
            identity.key()
        );
    }

    #[test]
    fn without_an_account_the_hostmask_is_the_identity() {
        let identity = UserIdentity::from_message(&message(&[])).expect("the sender is known");

        assert!(!identity.is_account());
        assert_eq!(identity.key(), "hostmask:user@host");
        assert_eq!(identity.hostmask(), "nick!user@host");
    }

    #[test]
    fn a_star_account_placeholder_reads_as_no_account() {
        let identity =
            UserIdentity::from_message(&message(&[("account", "*")])).expect("the sender is known");

        assert!(!identity.is_account());
        assert_eq!(identity.key(), "hostmask:user@host");
    }

    #[test]
    fn an_account_and_a_hostmask_spelling_the_same_string_never_collide() {
        let accounted = UserIdentity::from_parts(Some("user@host"), Sender::new("n", "u", "h"));
        let hostmask = UserIdentity::from_parts(None, Sender::new("n", "user", "host"));

        assert_ne!(accounted.key(), hostmask.key());
    }

    #[test]
    fn messages_without_a_prefix_have_no_identity() {
        let message = Message::new(None, "PING", vec!["cookie"]).expect("the message should parse");

        assert!(UserIdentity::from_message(&message).is_none());
    }
}
