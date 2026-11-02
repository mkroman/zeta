//! Admin authorization by IRC hostmask.
//!
//! Plugins that manage state through admin-only commands (`.filter`, `.unwall`) authorize the
//! sender by matching their `nick!user@host` against the wildcard patterns configured in
//! `[irc] admin_hostmasks`. With no hostmasks configured, nobody is an admin.

use tracing::{debug, warn};
use wildmatch::WildMatch;

use crate::event::Sender;

/// The compiled admin hostmasks of `[irc] admin_hostmasks`.
#[derive(Debug, Default)]
pub struct Hostmasks(Vec<WildMatch>);

impl Hostmasks {
    /// Compiles the configured hostmasks into case-insensitive wildcard matchers, skipping
    /// blank entries.
    ///
    /// `action` names what the hostmasks authorize — e.g. `"manage filters"` — for the log
    /// line emitted when none are configured.
    #[must_use]
    pub fn compile(hostmasks: &[String], action: &str) -> Self {
        let patterns: Vec<WildMatch> = hostmasks
            .iter()
            .map(|hostmask| hostmask.trim())
            .filter(|hostmask| !hostmask.is_empty())
            .map(WildMatch::new_case_insensitive)
            .collect();

        if patterns.is_empty() {
            warn!(
                "no admin hostmasks configured; nobody is authorized to {action} \
                 (set [irc] admin_hostmasks)"
            );
        } else {
            debug!(
                zeta.admin.hostmask.count = patterns.len(),
                "loaded admin hostmasks"
            );
        }

        Self(patterns)
    }

    /// Whether `sender`'s `nick!user@host` matches any of the hostmasks.
    #[must_use]
    pub fn matches(&self, sender: Sender<'_>) -> bool {
        let needed = sender.nick.len() + sender.username.len() + sender.hostname.len() + 2;

        if needed > Self::HOSTMASK_CAPACITY {
            let hostmask = format!("{}!{}@{}", sender.nick, sender.username, sender.hostname);

            return self.0.iter().any(|pattern| pattern.matches(&hostmask));
        }

        let nick = sender.nick.as_bytes();
        let username = sender.username.as_bytes();
        let hostname = sender.hostname.as_bytes();

        let mut buffer = [0_u8; Self::HOSTMASK_CAPACITY];
        let user_start = nick.len() + 1;
        let host_start = user_start + username.len() + 1;
        buffer[..nick.len()].copy_from_slice(nick);
        buffer[nick.len()] = b'!';
        buffer[user_start..host_start - 1].copy_from_slice(username);
        buffer[host_start - 1] = b'@';
        buffer[host_start..needed].copy_from_slice(hostname);

        // The components are `&str`s concatenated on code-point boundaries, so the result is
        // valid UTF-8 by construction; the fallback keeps the function total anyway.
        let Ok(hostmask) = std::str::from_utf8(&buffer[..needed]) else {
            let hostmask = format!("{}!{}@{}", sender.nick, sender.username, sender.hostname);

            return self.0.iter().any(|pattern| pattern.matches(&hostmask));
        };

        self.0.iter().any(|pattern| pattern.matches(hostmask))
    }

    /// The stack buffer a `nick!user@host` mask is assembled in.
    ///
    /// RFC 2812 caps a nickname at 32 bytes and common ircds a hostname at 64, so every mask a
    /// server sends fits; a longer one falls back to `format!`.
    const HOSTMASK_CAPACITY: usize = 128;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostmasks_match_wildcards_and_case_insensitively() {
        let admins = Hostmasks::compile(
            &["mk!mk@*".to_string(), "*!*@example.org".to_string()],
            "test",
        );

        assert!(admins.matches(Sender::new("mk", "mk", "user.example")));
        assert!(admins.matches(Sender::new("MK", "MK", "anything.tld")));
        assert!(admins.matches(Sender::new("someone", "x", "EXAMPLE.ORG")));
        assert!(!admins.matches(Sender::new("someone", "x", "example.com")));
        assert!(!admins.matches(Sender::new("mk", "other", "user.example")));
    }

    #[test]
    fn overlong_masks_take_the_allocating_fallback() {
        let admins = Hostmasks::compile(&["*!*@*long.example".to_string()], "test");
        let hostname = format!("{}.long.example", "a".repeat(120));

        // The assembled mask is longer than the stack buffer, so the wildcard match must still
        // see the full `nick!user@host`.
        assert!(admins.matches(Sender::new("nick", "user", &hostname)));
    }

    #[test]
    fn an_empty_list_denies_everyone() {
        let admins = Hostmasks::compile(&[], "test");

        assert!(!admins.matches(Sender::new("mk", "mk", "example.com")));
    }

    #[test]
    fn blank_hostmasks_are_skipped() {
        let admins = Hostmasks::compile(&["  ".to_string(), String::new()], "test");

        assert!(!admins.matches(Sender::new("mk", "mk", "example.com")));
    }
}
