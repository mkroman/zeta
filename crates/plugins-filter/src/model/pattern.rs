//! Wildcard pattern compilation and matching.

use wildmatch::WildMatch;

/// Whether `pattern` contains wildcards.
#[must_use]
pub(crate) fn is_wildcard(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

/// The case sensitivity of a wildcard pattern comparison.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Case {
    /// URL paths are matched case-sensitively.
    Sensitive,
    /// Every other field is matched case-insensitively.
    Insensitive,
}

/// Which end of a value the literal part of an affix pattern is compared against.
#[derive(Debug, Clone, Copy)]
pub(super) enum Anchor {
    /// `prefix*`
    Prefix,
    /// `*suffix`
    Suffix,
}

/// Compares `value` against `text` as `anchor` and `case` describe. `value` must be ASCII, so
/// every byte index is a character boundary.
fn match_affix(value: &str, text: &str, case: Case, anchor: Anchor) -> bool {
    let compared = match anchor {
        Anchor::Prefix => value.get(..text.len()),
        Anchor::Suffix => value
            .len()
            .checked_sub(text.len())
            .map(|start| &value[start..]),
    };

    compared.is_some_and(|compared| match case {
        Case::Sensitive => compared == text,
        Case::Insensitive => compared.eq_ignore_ascii_case(text),
    })
}

/// Returns the literal part of a single-sided `*suffix` or `prefix*` pattern and its anchor.
///
/// Only an ASCII pattern with exactly one `*` at one end qualifies; any other wildcard keeps the
/// general matcher.
fn affix_pattern(pattern: &str) -> Option<(&str, Anchor)> {
    if !pattern.is_ascii() {
        return None;
    }

    if let Some(suffix) = pattern.strip_prefix('*').filter(|text| !is_wildcard(text)) {
        return Some((suffix, Anchor::Suffix));
    }

    pattern
        .strip_suffix('*')
        .filter(|text| !is_wildcard(text))
        .map(|prefix| (prefix, Anchor::Prefix))
}

/// The compiled matcher of a [`Pattern`].
#[derive(Debug, Clone, Default)]
pub(super) enum Matcher {
    /// The pattern is unset or blank, and matches everything.
    #[default]
    Unset,
    /// A literal pattern without wildcards.
    Literal {
        /// The pattern text; lowercased for case-insensitive patterns.
        text: String,
        /// How the comparison folds case.
        case: Case,
    },
    /// A `*suffix` or `prefix*` pattern with no other wildcard.
    ///
    /// A single `*` at one end only needs a prefix or suffix comparison, which is cheaper than
    /// the backtracking [`WildMatch`]; the general matcher is kept for non-ASCII values. It is
    /// boxed to keep the variant small: the fallback only runs for non-ASCII values, which no
    /// ASCII comparison can fold the way [`WildMatch`] does.
    Affix {
        /// The literal part; lowercased for case-insensitive patterns.
        text: String,
        /// How the comparison folds case.
        case: Case,
        /// Which end of the value the literal is anchored to.
        anchor: Anchor,
        /// The general matcher, used for non-ASCII values.
        fallback: Box<WildMatch>,
    },
    /// A wildcard pattern.
    Wild(WildMatch),
}

/// A wildcard pattern compiled once for repeated matching.
///
/// A missing or blank pattern is unset and matches everything, mirroring the filter semantics.
#[derive(Debug, Clone, Default)]
pub(crate) struct Pattern {
    /// The compiled matcher.
    pub(super) matcher: Matcher,
}

impl Pattern {
    /// Compiles `raw`, trimming it and treating a blank pattern as unset.
    #[must_use]
    pub(crate) fn compile(raw: Option<&str>, case: Case) -> Self {
        let matcher = raw
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .map_or(Matcher::Unset, |pattern| {
                if pattern.is_ascii() && !is_wildcard(pattern) {
                    Matcher::Literal {
                        text: match case {
                            Case::Sensitive => pattern.to_owned(),
                            Case::Insensitive => pattern.to_ascii_lowercase(),
                        },
                        case,
                    }
                } else if let Some((text, anchor)) = affix_pattern(pattern) {
                    Matcher::Affix {
                        text: match case {
                            Case::Sensitive => text.to_owned(),
                            Case::Insensitive => text.to_ascii_lowercase(),
                        },
                        case,
                        anchor,
                        fallback: Box::new(match case {
                            Case::Sensitive => WildMatch::new(pattern),
                            Case::Insensitive => WildMatch::new_case_insensitive(pattern),
                        }),
                    }
                } else {
                    Matcher::Wild(match case {
                        Case::Sensitive => WildMatch::new(pattern),
                        Case::Insensitive => WildMatch::new_case_insensitive(pattern),
                    })
                }
            });

        Self { matcher }
    }

    /// Whether the pattern is unset.
    #[must_use]
    pub(crate) const fn is_unset(&self) -> bool {
        matches!(&self.matcher, Matcher::Unset)
    }

    /// Whether the pattern matches `value`, or is unset (an unset criterion matches anything).
    #[must_use]
    pub(crate) fn matches(&self, value: Option<&str>) -> bool {
        let Some(value) = value else {
            return self.is_unset();
        };

        match &self.matcher {
            Matcher::Unset => true,
            Matcher::Literal { text, case } => match case {
                Case::Sensitive => value == text.as_str(),
                Case::Insensitive if value.is_ascii() => value.eq_ignore_ascii_case(text),
                Case::Insensitive => {
                    // `WildMatch` lowercases Unicode character-wise, so an ASCII pattern can
                    // match non-ASCII input (the Kelvin sign lowercases to `k`); mirror that
                    // instead of comparing ASCII only, without allocating a lowered copy.
                    value.chars().flat_map(char::to_lowercase).eq(text.chars())
                }
            },
            Matcher::Affix {
                text,
                case,
                anchor,
                fallback,
            } => {
                // `WildMatch` folds case per character; an ASCII comparison does not reproduce
                // that for non-ASCII input, so only ASCII values take the fast path.
                if value.is_ascii() {
                    match_affix(value, text, *case, *anchor)
                } else {
                    fallback.matches(value)
                }
            }
            Matcher::Wild(pattern) => pattern.matches(value),
        }
    }
}
