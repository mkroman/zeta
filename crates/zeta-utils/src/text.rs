//! Text shaping, joining, and matching helpers.
//!
//! The module is split by concern: `truncate` cuts text to a character budget, `case` matches
//! prefixes and suffixes without regard to case, `literal` locates quoted string literals,
//! `join` builds English sentences from lists and iterators, `numbers` formats counts and
//! pluralizes nouns, `article` selects the indefinite article, and this module collapses
//! whitespace. Every public item is re-exported here, so `zeta_utils::text::{…}` stays the
//! import path for all of them.

mod article;
mod case;
mod join;
mod literal;
mod numbers;
mod truncate;

pub use article::indefinite_article;
pub use case::{strip_prefix_ignore_case, strip_suffix_ignore_case};
pub use join::{IteratorJoinExt, ListJoinExt};
pub use literal::quoted_string_literal;
pub use numbers::{format_number, pluralize, pluralize_noun};
pub use truncate::TruncatableExt;

/// Collapses runs of whitespace in `value` into single spaces, trimming the result.
///
/// Text assembled from web pages carries line breaks and repeated spaces that an IRC message
/// cannot render: a line break terminates the message, and everything after it is lost.
#[must_use]
pub fn collapse_whitespace(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut first = true;

    for word in value.split_whitespace() {
        if !first {
            collapsed.push(' ');
        }

        collapsed.push_str(word);
        first = false;
    }

    collapsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_whitespace_runs_into_single_spaces() {
        // A line break terminates an IRC message, so text from web pages is collapsed.
        assert_eq!(
            collapse_whitespace("first\nsecond\nthird"),
            "first second third"
        );
        assert_eq!(collapse_whitespace("  a   b\t\tc  "), "a b c");
        assert_eq!(collapse_whitespace("plain"), "plain");
        assert_eq!(collapse_whitespace("   "), "");
    }
}
