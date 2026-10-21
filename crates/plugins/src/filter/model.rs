//! The data model of the filter plugin.

use sqlx::prelude::FromRow;

use super::pattern::{Case, Pattern};

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
pub struct FilterRule {
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
pub struct NewFilterRule {
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

    /// Compiles the set criteria into wildcard matchers, so selecting filters compiles every
    /// pattern once instead of once per filter.
    pub(super) fn compile(&self) -> CompiledCriteria {
        CompiledCriteria {
            channel: Pattern::parse(self.channel.as_deref(), Case::Insensitive),
            host: Pattern::parse(self.host.as_deref(), Case::Insensitive),
            path: Pattern::parse(self.path.as_deref(), Case::Sensitive),
            nickname: Pattern::parse(self.nickname.as_deref(), Case::Insensitive),
            username: Pattern::parse(self.username.as_deref(), Case::Insensitive),
            hostname: Pattern::parse(self.hostname.as_deref(), Case::Insensitive),
        }
    }
}

/// Filter criteria compiled into wildcard matchers.
///
/// Built with [`Criteria::compile`]; a filter is selected when all of the set patterns match
/// its corresponding field.
#[derive(Debug)]
pub(super) struct CompiledCriteria {
    /// The compiled channel pattern, if any.
    channel: Option<Pattern>,
    /// The compiled URL host pattern, if any.
    host: Option<Pattern>,
    /// The compiled URL path pattern, if any.
    path: Option<Pattern>,
    /// The compiled sender nickname pattern, if any.
    nickname: Option<Pattern>,
    /// The compiled sender username pattern, if any.
    username: Option<Pattern>,
    /// The compiled sender hostname pattern, if any.
    hostname: Option<Pattern>,
}

impl CompiledCriteria {
    /// Whether the criteria select `filter`.
    pub(super) fn matches(&self, filter: &FilterRule) -> bool {
        Self::field_matches(self.channel.as_ref(), filter.channel.as_deref())
            && Self::field_matches(self.host.as_ref(), filter.host.as_deref())
            && Self::field_matches(self.path.as_ref(), filter.path.as_deref())
            && Self::field_matches(self.nickname.as_ref(), filter.nickname.as_deref())
            && Self::field_matches(self.username.as_ref(), filter.username.as_deref())
            && Self::field_matches(self.hostname.as_ref(), filter.hostname.as_deref())
    }

    /// Whether `pattern` matches `value`, or is unset.
    fn field_matches(pattern: Option<&Pattern>, value: Option<&str>) -> bool {
        pattern.is_none_or(|pattern| value.is_some_and(|value| pattern.matches(value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(host: Option<&str>, path: Option<&str>) -> FilterRule {
        FilterRule {
            id: 1,
            channel: Some("#foo".into()),
            host: host.map(String::from),
            path: path.map(String::from),
            nickname: None,
            username: None,
            hostname: None,
        }
    }

    /// Whether `criteria` selects `filter`.
    fn matches(criteria: &Criteria, filter: &FilterRule) -> bool {
        criteria.compile().matches(filter)
    }

    #[test]
    fn criteria_match_selectively() {
        let criteria = Criteria {
            host: Some("*.com".into()),
            ..Criteria::default()
        };

        assert!(matches(&criteria, &filter(Some("reuters.com"), None)));
        assert!(!matches(&criteria, &filter(Some("dr.dk"), None)));
    }

    #[test]
    fn empty_criteria_match_everything() {
        let criteria = Criteria::default();

        assert!(matches(&criteria, &filter(None, None)));
    }

    #[test]
    fn blank_criteria_match_everything() {
        let criteria = Criteria {
            host: Some("  ".into()),
            ..Criteria::default()
        };

        assert!(matches(&criteria, &filter(None, None)));
    }

    #[test]
    fn set_criteria_do_not_match_unset_fields() {
        let criteria = Criteria {
            host: Some("dr.dk".into()),
            ..Criteria::default()
        };

        assert!(!matches(&criteria, &filter(None, None)));
    }

    #[test]
    fn path_criteria_are_case_sensitive() {
        let upper = Criteria {
            path: Some("/Title/*".into()),
            ..Criteria::default()
        };

        assert!(!matches(&upper, &filter(None, Some("/title/*"))));

        let lower = Criteria {
            path: Some("/title/*".into()),
            ..Criteria::default()
        };

        assert!(matches(&lower, &filter(None, Some("/title/*"))));
    }

    #[test]
    fn host_criteria_are_case_insensitive() {
        let criteria = Criteria {
            host: Some("DR.DK".into()),
            ..Criteria::default()
        };

        assert!(matches(&criteria, &filter(Some("dr.dk"), None)));
    }
}
