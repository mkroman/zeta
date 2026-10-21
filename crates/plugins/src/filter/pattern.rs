//! Wildcard patterns shared by the filter index and the selection criteria.

use wildmatch::WildMatch;

/// The case sensitivity of a wildcard comparison.
#[derive(Debug, Clone, Copy)]
pub(super) enum Case {
    /// URL paths are matched case-sensitively.
    Sensitive,
    /// Every other field is matched case-insensitively.
    Insensitive,
}

/// A wildcard pattern compiled once for a fixed case sensitivity.
///
/// Patterns are compiled when a filter is indexed and when the `list` and `delete` criteria are
/// compiled, never on the matching path.
#[derive(Debug, Clone)]
pub(super) struct Pattern {
    /// The compiled matcher.
    matcher: WildMatch,
}

impl Pattern {
    /// Compiles `pattern` for comparisons with `case`.
    #[must_use]
    pub(super) fn new(pattern: &str, case: Case) -> Self {
        let matcher = match case {
            Case::Sensitive => WildMatch::new(pattern),
            Case::Insensitive => WildMatch::new_case_insensitive(pattern),
        };

        Self { matcher }
    }

    /// Compiles `pattern` for comparisons with `case`, treating a missing or blank pattern as
    /// unset.
    #[must_use]
    pub(super) fn parse(pattern: Option<&str>, case: Case) -> Option<Self> {
        non_blank(pattern).map(|pattern| Self::new(pattern, case))
    }

    /// Whether the pattern matches `value`.
    #[must_use]
    pub(super) fn matches(&self, value: &str) -> bool {
        self.matcher.matches(value)
    }
}

/// Whether `pattern` contains wildcards.
#[must_use]
pub(super) fn has_wildcards(pattern: &str) -> bool {
    pattern.contains(['*', '?'])
}

/// Returns `pattern` trimmed, or `None` when it is missing or blank.
#[must_use]
pub(super) fn non_blank(pattern: Option<&str>) -> Option<&str> {
    pattern.map(str::trim).filter(|pattern| !pattern.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_with_the_requested_case_sensitivity() {
        assert!(Pattern::new("*.COM", Case::Insensitive).matches("reuters.com"));
        assert!(!Pattern::new("*.COM", Case::Sensitive).matches("reuters.com"));
    }

    #[test]
    fn blank_patterns_are_unset() {
        assert!(Pattern::parse(Some("  "), Case::Sensitive).is_none());
        assert!(Pattern::parse(None, Case::Sensitive).is_none());
    }

    #[test]
    fn detects_wildcards() {
        assert!(has_wildcards("*.com"));
        assert!(has_wildcards("a?b"));
        assert!(!has_wildcards("imdb.com"));
    }
}
