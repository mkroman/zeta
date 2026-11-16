//! Criteria for selecting filters in listings and deletions.

use super::filter::Filter;
use super::pattern::{Case, Pattern};

/// A set of filter criteria for selecting filters in listings and deletions.
///
/// Every criterion is optional; a filter is selected when all of the set criteria match its
/// corresponding field. Wildcards are supported in every criterion; paths are compared
/// case-sensitively, everything else case-insensitively.
#[derive(Debug, Default, Clone)]
pub struct Criteria {
    /// The channel pattern, or `None` for any channel.
    pub channel: Option<String>,
    /// The URL host pattern, or `None` for any host.
    pub host: Option<String>,
    /// The URL path pattern, or `None` for any path.
    pub path: Option<String>,
    /// The sender nickname pattern, or `None` for any nickname.
    pub nickname: Option<String>,
    /// The sender username pattern, or `None` for any username.
    pub username: Option<String>,
    /// The sender hostname pattern, or `None` for any hostname.
    pub hostname: Option<String>,
}

impl Criteria {
    /// Whether no criterion is set.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.channel.is_none()
            && self.host.is_none()
            && self.path.is_none()
            && self.nickname.is_none()
            && self.username.is_none()
            && self.hostname.is_none()
    }

    /// Compiles the patterns once, for matching many filters.
    #[must_use]
    pub fn compile(&self) -> CompiledCriteria {
        CompiledCriteria {
            channel: Pattern::compile(self.channel.as_deref(), Case::Insensitive),
            host: Pattern::compile(self.host.as_deref(), Case::Insensitive),
            path: Pattern::compile(self.path.as_deref(), Case::Sensitive),
            nickname: Pattern::compile(self.nickname.as_deref(), Case::Insensitive),
            username: Pattern::compile(self.username.as_deref(), Case::Insensitive),
            hostname: Pattern::compile(self.hostname.as_deref(), Case::Insensitive),
        }
    }
}

/// Criteria with their wildcard patterns compiled once.
#[derive(Debug, Clone)]
pub struct CompiledCriteria {
    /// The compiled channel pattern.
    channel: Pattern,
    /// The compiled host pattern.
    host: Pattern,
    /// The compiled path pattern.
    path: Pattern,
    /// The compiled nickname pattern.
    nickname: Pattern,
    /// The compiled username pattern.
    username: Pattern,
    /// The compiled hostname pattern.
    hostname: Pattern,
}

impl CompiledCriteria {
    /// Whether the criteria select `filter`.
    #[must_use]
    pub fn matches(&self, filter: &Filter) -> bool {
        self.channel.matches(filter.channel.as_deref())
            && self.host.matches(filter.host.as_deref())
            && self.path.matches(filter.path.as_deref())
            && self.nickname.matches(filter.nickname.as_deref())
            && self.username.matches(filter.username.as_deref())
            && self.hostname.matches(filter.hostname.as_deref())
    }
}
