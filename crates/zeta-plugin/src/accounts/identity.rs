//! The identity of a message's sender.

use irc::proto::Message;

use crate::event::{Sender, account};

/// The stable identity of a message's sender, for everything the bot tracks per user.
///
/// Identities are services accounts, resolved from the IRCv3 `account` tag: the tag is what the
/// network reports, so the identity survives reconnects, nick changes and new hosts, and a user
/// who has not identified has no identity at all — per-user state simply does not apply to
/// them. Hostmasks are carried alongside for tracking (see
/// [`Accounts`](super::Accounts)): accounts move across hosts, so which hosts used an account
/// is exactly what the tracking answers.
///
/// Built by the generated `identity()` accessor on every event; constructed directly in tests
/// and from already-parsed parts through [`UserIdentity::from_parts`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserIdentity {
    /// The account name, lowercased.
    key: String,
    /// The sender's `nick!user@host` at the time of the message.
    hostmask: String,
}

impl UserIdentity {
    /// Resolves the identity of `message`'s sender, if it is logged into services.
    ///
    /// The `account` tag decides: senders who have not identified (no tag, a `*` placeholder or
    /// an empty value — networks differ in which of the three they send) have no identity, and
    /// per-user state does not apply to them.
    #[must_use]
    pub fn from_message(message: &Message) -> Option<Self> {
        let sender = Sender::from_message(message)?;
        let account = account(message)?;

        Some(Self::from_parts(account, sender))
    }

    /// Builds the identity from a services account and the sender's prefix parts.
    ///
    /// The hostmask is tracked alongside the account: accounts move across hosts, so which
    /// hosts used an account is exactly what the tracking answers.
    #[must_use]
    pub fn from_parts(account: &str, sender: Sender<'_>) -> Self {
        Self {
            key: account.trim().to_ascii_lowercase(),
            hostmask: format!("{}!{}@{}", sender.nick, sender.username, sender.hostname),
        }
    }

    /// The account name the database rows are stored under, lowercased.
    ///
    /// Accounts are case-insensitive in the services that issue them, so `Alice` and `alice`
    /// are one identity.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The sender's `nick!user@host` at the time of the message.
    ///
    /// Recorded for every identity, so the hosts an account has been used from stay auditable
    /// even though the account itself moves across hosts.
    #[must_use]
    pub fn hostmask(&self) -> &str {
        &self.hostmask
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
    fn an_account_tag_identifies_the_sender() {
        let identity = UserIdentity::from_message(&message(&[("account", "alice")]))
            .expect("the sender is identified");

        assert_eq!(identity.key(), "alice");
        assert_eq!(identity.hostmask(), "nick!user@host");

        // Account names are case-insensitive, like the services that issue them.
        assert_eq!(
            UserIdentity::from_parts("ALICE", Sender::new("n", "u", "h")).key(),
            identity.key()
        );
    }

    #[test]
    fn a_sender_who_has_not_identified_has_no_identity() {
        // Networks differ in what they report for an unidentified sender: no tag at all, an
        // empty tag, or the `*` placeholder. None of them is an identity.
        assert!(UserIdentity::from_message(&message(&[])).is_none());
        assert!(UserIdentity::from_message(&message(&[("account", "")])).is_none());
        assert!(UserIdentity::from_message(&message(&[("account", "*")])).is_none());
    }

    #[test]
    fn messages_without_a_prefix_have_no_identity() {
        let message = Message::new(None, "PING", vec!["cookie"]).expect("the message should parse");

        assert!(UserIdentity::from_message(&message).is_none());
    }
}
