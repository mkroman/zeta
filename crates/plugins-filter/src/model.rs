//! The data model of the filter plugin.

use sqlx::prelude::FromRow;
use wildmatch::WildMatch;

/// A filter, as stored in the database.
///
/// Every criterion is optional; a filter matches a message when all of its set criteria match —
/// the channel the message was posted in, the host and path of a URL in it, and the nickname,
/// username (ident), and hostname of its sender. Patterns may contain `*` and `?` wildcards;
/// paths are matched case-sensitively, everything else case-insensitively.
///
/// The creator nickname and creation time columns are written on insert but not part of the
/// row model, which only carries what the plugin reads.
#[derive(Debug, FromRow, Clone, PartialEq, Eq)]
pub struct Filter {
    /// The database id of the filter.
    pub id: i32,
    /// The channel the filter applies to, or `None` for all channels.
    pub channel: Option<String>,
    /// The URL host pattern, or `None` for any host.
    pub host: Option<String>,
    /// The URL path pattern, or `None` for any path.
    pub path: Option<String>,
    /// The sender nickname pattern, or `None` for any nickname.
    pub nickname: Option<String>,
    /// The sender username (ident) pattern, or `None` for any username.
    pub username: Option<String>,
    /// The sender hostname pattern, or `None` for any hostname.
    pub hostname: Option<String>,
}

/// A filter to be inserted into the database.
#[derive(Debug, Clone)]
pub struct NewFilter {
    /// The channel the filter applies to, or `None` for all channels.
    pub channel: Option<String>,
    /// The URL host pattern, or `None` for any host.
    pub host: Option<String>,
    /// The URL path pattern, or `None` for any path.
    pub path: Option<String>,
    /// The sender nickname pattern, or `None` for any nickname.
    pub nickname: Option<String>,
    /// The sender username (ident) pattern, or `None` for any username.
    pub username: Option<String>,
    /// The sender hostname pattern, or `None` for any hostname.
    pub hostname: Option<String>,
    /// The nickname of the user creating the filter.
    pub created_by: String,
}

/// The case sensitivity of a wildcard pattern comparison.
#[derive(Debug, Clone, Copy)]
pub enum Case {
    /// URL paths are matched case-sensitively.
    Sensitive,
    /// Every other field is matched case-insensitively.
    Insensitive,
}

/// A wildcard pattern compiled once for repeated matching.
///
/// A missing or blank pattern is unset and matches everything, mirroring the filter semantics.
#[derive(Debug, Clone, Default)]
pub struct Pattern {
    /// The compiled matcher, or `None` when the pattern is unset or blank.
    matcher: Option<WildMatch>,
}

impl Pattern {
    /// Compiles `raw`, trimming it and treating a blank pattern as unset.
    #[must_use]
    pub fn compile(raw: Option<&str>, case: Case) -> Self {
        let matcher = raw
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .map(|pattern| match case {
                Case::Sensitive => WildMatch::new(pattern),
                Case::Insensitive => WildMatch::new_case_insensitive(pattern),
            });

        Self { matcher }
    }

    /// Whether the pattern is unset.
    #[must_use]
    pub const fn is_unset(&self) -> bool {
        self.matcher.is_none()
    }

    /// Whether the pattern matches `value`, or is unset (an unset criterion matches anything).
    #[must_use]
    pub fn matches(&self, value: Option<&str>) -> bool {
        self.matcher
            .as_ref()
            .is_none_or(|pattern| value.is_some_and(|value| pattern.matches(value)))
    }
}

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

    /// Whether the criteria select `filter`.
    ///
    /// Compiles the patterns on every call; use [`Criteria::compile`] when matching a list.
    #[must_use]
    pub fn matches(&self, filter: &Filter) -> bool {
        self.compile().matches(filter)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(host: Option<&str>, path: Option<&str>) -> Filter {
        Filter {
            id: 1,
            channel: Some("#foo".into()),
            host: host.map(String::from),
            path: path.map(String::from),
            nickname: None,
            username: None,
            hostname: None,
        }
    }

    impl Filter {
        /// Builds a filter from the fields of `new`, for tests that need one without a database.
        pub(crate) fn from_new_filter(id: i32, new: NewFilter) -> Self {
            Self {
                id,
                channel: new.channel,
                host: new.host,
                path: new.path,
                nickname: new.nickname,
                username: new.username,
                hostname: new.hostname,
            }
        }
    }

    #[test]
    fn criteria_match_selected_fields() {
        let cases = [
            (
                "a matching wildcard host",
                Criteria {
                    host: Some("*.com".into()),
                    ..Criteria::default()
                },
                filter(Some("reuters.com"), None),
                true,
            ),
            (
                "a non-matching wildcard host",
                Criteria {
                    host: Some("*.com".into()),
                    ..Criteria::default()
                },
                filter(Some("dr.dk"), None),
                false,
            ),
            (
                "empty criteria",
                Criteria::default(),
                filter(None, None),
                true,
            ),
            (
                "a set host against an unset field",
                Criteria {
                    host: Some("dr.dk".into()),
                    ..Criteria::default()
                },
                filter(None, None),
                false,
            ),
        ];

        for (case, criteria, filter, expected) in cases {
            assert_eq!(criteria.matches(&filter), expected, "for {case}");
        }
    }

    #[test]
    fn criteria_matching_respects_path_case_sensitivity() {
        let upper_path = Criteria {
            path: Some("/Title/*".into()),
            ..Criteria::default()
        };
        let lower_path = Criteria {
            path: Some("/title/*".into()),
            ..Criteria::default()
        };
        let upper_host = Criteria {
            host: Some("DR.DK".into()),
            ..Criteria::default()
        };

        assert!(!upper_path.matches(&filter(None, Some("/title/*"))));
        assert!(lower_path.matches(&filter(None, Some("/title/*"))));
        assert!(upper_host.matches(&filter(Some("dr.dk"), None)));
    }
}
