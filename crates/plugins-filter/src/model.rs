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

/// Which end of a value the literal part of an affix pattern is compared against.
#[derive(Debug, Clone, Copy)]
enum Anchor {
    /// `prefix*`
    Prefix,
    /// `*suffix`
    Suffix,
}

/// Compares `value` against `text` as `anchor` describes. `value` must be ASCII, so every byte
/// index is a character boundary.
fn match_affix(value: &str, text: &str, sensitive: bool, anchor: Anchor) -> bool {
    let compared = match anchor {
        Anchor::Prefix => value.get(..text.len()),
        Anchor::Suffix => value
            .len()
            .checked_sub(text.len())
            .map(|start| &value[start..]),
    };

    compared.is_some_and(|compared| {
        if sensitive {
            compared == text
        } else {
            compared.eq_ignore_ascii_case(text)
        }
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

    if let Some(suffix) = pattern
        .strip_prefix('*')
        .filter(|text| !text.contains(['*', '?']))
    {
        return Some((suffix, Anchor::Suffix));
    }

    pattern
        .strip_suffix('*')
        .filter(|text| !text.contains(['*', '?']))
        .map(|prefix| (prefix, Anchor::Prefix))
}

/// The compiled matcher of a [`Pattern`].
#[derive(Debug, Clone, Default)]
enum Matcher {
    /// The pattern is unset or blank, and matches everything.
    #[default]
    Unset,
    /// A literal pattern without wildcards.
    Literal {
        /// The pattern text; lowercased for case-insensitive patterns.
        text: String,
        /// Whether the comparison is case-sensitive.
        sensitive: bool,
    },
    /// A `*suffix` or `prefix*` pattern with no other wildcard.
    ///
    /// A single `*` at one end only needs a prefix or suffix comparison, which is cheaper than
    /// the backtracking [`WildMatch`]; the general matcher is kept for non-ASCII values.
    Affix {
        /// The literal part; lowercased for case-insensitive patterns.
        text: String,
        /// Whether the comparison is case-sensitive.
        sensitive: bool,
        /// Which end of the value the literal is anchored to.
        anchor: Anchor,
        /// The general matcher, used for non-ASCII values.
        fallback: WildMatch,
    },
    /// A wildcard pattern.
    Wild(WildMatch),
}

/// A wildcard pattern compiled once for repeated matching.
///
/// A missing or blank pattern is unset and matches everything, mirroring the filter semantics.
#[derive(Debug, Clone, Default)]
pub struct Pattern {
    /// The compiled matcher.
    matcher: Matcher,
}

impl Pattern {
    /// Compiles `raw`, trimming it and treating a blank pattern as unset.
    #[must_use]
    pub fn compile(raw: Option<&str>, case: Case) -> Self {
        let matcher = raw
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .map_or(Matcher::Unset, |pattern| {
                if pattern.is_ascii() && !pattern.contains('*') && !pattern.contains('?') {
                    Matcher::Literal {
                        text: match case {
                            Case::Sensitive => pattern.to_owned(),
                            Case::Insensitive => pattern.to_ascii_lowercase(),
                        },
                        sensitive: matches!(case, Case::Sensitive),
                    }
                } else if let Some((text, anchor)) = affix_pattern(pattern) {
                    Matcher::Affix {
                        text: match case {
                            Case::Sensitive => text.to_owned(),
                            Case::Insensitive => text.to_ascii_lowercase(),
                        },
                        sensitive: matches!(case, Case::Sensitive),
                        anchor,
                        fallback: match case {
                            Case::Sensitive => WildMatch::new(pattern),
                            Case::Insensitive => WildMatch::new_case_insensitive(pattern),
                        },
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
    pub const fn is_unset(&self) -> bool {
        matches!(&self.matcher, Matcher::Unset)
    }

    /// Whether the pattern matches `value`, or is unset (an unset criterion matches anything).
    #[must_use]
    pub fn matches(&self, value: Option<&str>) -> bool {
        let Some(value) = value else {
            return self.is_unset();
        };

        match &self.matcher {
            Matcher::Unset => true,
            Matcher::Literal { text, sensitive } => {
                if *sensitive {
                    value == text.as_str()
                } else if value.is_ascii() {
                    value.eq_ignore_ascii_case(text)
                } else {
                    // `WildMatch` lowercases Unicode character-wise, so an ASCII pattern can
                    // match non-ASCII input (the Kelvin sign lowercases to `k`); mirror that
                    // instead of comparing ASCII only.
                    value.to_lowercase() == text.as_str()
                }
            }
            Matcher::Affix {
                text,
                sensitive,
                anchor,
                fallback,
            } => {
                // `WildMatch` folds case per character; an ASCII comparison does not reproduce
                // that for non-ASCII input, so only ASCII values take the fast path.
                if value.is_ascii() {
                    match_affix(value, text, *sensitive, *anchor)
                } else {
                    fallback.matches(value)
                }
            }
            Matcher::Wild(pattern) => pattern.matches(value),
        }
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
            assert_eq!(criteria.compile().matches(&filter), expected, "for {case}");
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

        assert!(
            !upper_path
                .compile()
                .matches(&filter(None, Some("/title/*")))
        );
        assert!(
            lower_path
                .compile()
                .matches(&filter(None, Some("/title/*")))
        );
        assert!(upper_host.compile().matches(&filter(Some("dr.dk"), None)));
    }

    #[test]
    fn literal_patterns_match_without_wildcards() {
        let insensitive = Pattern::compile(Some("IMDB.com"), Case::Insensitive);

        assert!(insensitive.matches(Some("imdb.com")));
        assert!(insensitive.matches(Some("IMDB.COM")));
        assert!(!insensitive.matches(Some("www.imdb.com")));
        assert!(!insensitive.matches(None));

        let sensitive = Pattern::compile(Some("/Title"), Case::Sensitive);

        assert!(sensitive.matches(Some("/Title")));
        assert!(!sensitive.matches(Some("/title")));
    }

    #[test]
    fn literal_patterns_fold_non_ascii_input_like_wildmatch() {
        // U+212A KELVIN SIGN lowercases to `k`, so a case-insensitive wildcard pattern matches
        // it; the literal fast path has to fold non-ASCII input the same way.
        let pattern = Pattern::compile(Some("kelvin"), Case::Insensitive);

        assert!(pattern.matches(Some("Kelvin")));
        assert!(pattern.matches(Some("\u{212A}elvin")));
        assert!(!pattern.matches(Some("kelvi")));
    }

    #[test]
    fn wildcard_patterns_keep_wildcard_matching() {
        let pattern = Pattern::compile(Some("a?c"), Case::Insensitive);

        assert!(pattern.matches(Some("abc")));
        assert!(pattern.matches(Some("a?c")));
        assert!(!pattern.matches(Some("ac")));

        let non_ascii = Pattern::compile(Some("İ*"), Case::Insensitive);

        assert!(non_ascii.matches(Some("İstanbul")));
    }

    #[test]
    fn affix_patterns_match_single_sided_wildcards() {
        let suffix = Pattern::compile(Some("*bot"), Case::Insensitive);

        assert!(matches!(&suffix.matcher, Matcher::Affix { .. }));
        assert!(suffix.matches(Some("mybot")));
        assert!(suffix.matches(Some("bot")));
        assert!(suffix.matches(Some("MYBOT")));
        assert!(!suffix.matches(Some("botx")));
        assert!(!suffix.matches(Some("mybotx")));
        assert!(!suffix.matches(Some("bo")));

        let prefix = Pattern::compile(Some("bot*"), Case::Sensitive);

        assert!(matches!(&prefix.matcher, Matcher::Affix { .. }));
        assert!(prefix.matches(Some("botx")));
        assert!(prefix.matches(Some("bot")));
        assert!(!prefix.matches(Some("xbot")));
        assert!(!prefix.matches(Some("BOTX")));
        assert!(!prefix.matches(Some("bo")));
    }

    #[test]
    fn affix_patterns_cover_the_empty_literal() {
        let pattern = Pattern::compile(Some("*"), Case::Insensitive);

        assert!(matches!(&pattern.matcher, Matcher::Affix { .. }));
        assert!(pattern.matches(Some("")));
        assert!(pattern.matches(Some("anything")));
    }

    #[test]
    fn patterns_with_other_wildcards_keep_the_general_matcher() {
        let infix = Pattern::compile(Some("a*b"), Case::Insensitive);

        assert!(matches!(&infix.matcher, Matcher::Wild(_)));
        assert!(infix.matches(Some("ab")));
        assert!(infix.matches(Some("axxb")));
        assert!(!infix.matches(Some("axx")));

        let single = Pattern::compile(Some("a?b"), Case::Insensitive);

        assert!(matches!(&single.matcher, Matcher::Wild(_)));
        assert!(single.matches(Some("axb")));
        assert!(!single.matches(Some("axxb")));

        let both_ends = Pattern::compile(Some("*a*"), Case::Insensitive);

        assert!(matches!(&both_ends.matcher, Matcher::Wild(_)));
        assert!(both_ends.matches(Some("a")));
        assert!(both_ends.matches(Some("xax")));
        assert!(!both_ends.matches(Some("x")));
    }

    #[test]
    fn affix_patterns_fall_back_to_wildmatch_for_non_ascii_values() {
        // `WildMatch` folds case per character, so an ASCII pattern can match non-ASCII input
        // (U+212A KELVIN SIGN lowercases to `k`); the wildcard fallback preserves that.
        let kelvin = Pattern::compile(Some("*k"), Case::Insensitive);

        assert!(matches!(&kelvin.matcher, Matcher::Affix { .. }));
        assert!(kelvin.matches(Some("\u{212A}")));
        assert!(!kelvin.matches(Some("\u{212A}x")));

        let prefix = Pattern::compile(Some("k*"), Case::Insensitive);

        assert!(matches!(&prefix.matcher, Matcher::Affix { .. }));
        assert!(prefix.matches(Some("\u{212A}elvin")));
        assert!(!prefix.matches(Some("x\u{212A}elvin")));
        assert!(prefix.matches(Some("Kelvin")));
        assert!(!prefix.matches(Some("elvin")));

        // A non-ASCII pattern does not qualify for the affix fast path, but still matches.
        let non_ascii = Pattern::compile(Some("*é"), Case::Insensitive);

        assert!(matches!(&non_ascii.matcher, Matcher::Wild(_)));
        assert!(non_ascii.matches(Some("café")));
        assert!(!non_ascii.matches(Some("cafe")));
    }

    #[test]
    fn unset_and_blank_patterns_match_everything() {
        for raw in [None, Some(""), Some("   "), Some("\t\n")] {
            let pattern = Pattern::compile(raw, Case::Insensitive);

            assert!(pattern.is_unset(), "for {raw:?}");
            assert!(pattern.matches(Some("anything")), "for {raw:?}");
            assert!(pattern.matches(Some("")), "for {raw:?}");
            assert!(pattern.matches(None), "for {raw:?}");
        }
    }

    /// Times literal, affix, and general wildcard patterns.
    ///
    /// Ignored by default; run with
    /// `cargo test -p plugins-filter --lib -- --ignored --nocapture benchmark_affix_patterns`.
    /// The numbers are only meaningful in a release build; a debug run shows that every case
    /// executes, not how fast it is.
    #[test]
    #[ignore = "benchmark; run with --ignored --nocapture"]
    fn benchmark_affix_patterns() {
        use std::hint::black_box;
        use std::time::Instant;

        #[allow(clippy::cast_precision_loss)] // iteration counts are far below 2^52
        fn measure(mut operation: impl FnMut() -> bool) -> f64 {
            const ITERATIONS: usize = 100_000;
            const RUNS: usize = 5;

            let mut best = f64::MAX;

            for _ in 0..RUNS {
                let start = Instant::now();

                for _ in 0..ITERATIONS {
                    black_box(operation());
                }

                let nanos = start.elapsed().as_secs_f64() * 1e9;

                best = best.min(nanos / ITERATIONS as f64);
            }

            best
        }

        let cases = [
            (
                "literal",
                Pattern::compile(Some("someone"), Case::Insensitive),
                "someone",
            ),
            (
                "suffix affix",
                Pattern::compile(Some("*bot"), Case::Insensitive),
                "mybot",
            ),
            (
                "prefix affix",
                Pattern::compile(Some("bot*"), Case::Insensitive),
                "botx",
            ),
            (
                "affix miss",
                Pattern::compile(Some("*bot"), Case::Insensitive),
                "someone",
            ),
            (
                "general wildcard",
                Pattern::compile(Some("a*b"), Case::Insensitive),
                "axxb",
            ),
            (
                "non-ascii fallback",
                Pattern::compile(Some("*k"), Case::Insensitive),
                "\u{212A}",
            ),
        ];

        for (label, pattern, value) in cases {
            // Keep the inputs opaque: with literals the compiler can constant-fold the matching
            // a real lookup does, hiding the work the benchmark is meant to measure.
            let pattern = black_box(&pattern);
            let value = black_box(value);
            let nanos = measure(|| pattern.matches(Some(value)));

            println!("affix benchmark: {label}: {value:?}: {nanos:.1} ns/op");
        }
    }
}
