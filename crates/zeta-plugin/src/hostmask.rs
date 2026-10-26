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
            debug!(count = patterns.len(), "loaded admin hostmasks");
        }

        Self(patterns)
    }

    /// Whether `sender`'s `nick!user@host` matches any of the hostmasks.
    #[must_use]
    pub fn matches(&self, sender: Sender<'_>) -> bool {
        let hostmask = format!("{}!{}@{}", sender.nick, sender.username, sender.hostname);

        self.0.iter().any(|pattern| pattern.matches(&hostmask))
    }
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
