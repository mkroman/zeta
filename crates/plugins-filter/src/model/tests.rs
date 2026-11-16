//! Tests for the filter model: criteria selection and pattern matching.

use super::pattern::{Case, Matcher};
use super::{Criteria, Filter, NewFilter, Pattern};

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

    assert!(matches!(
        &insensitive.matcher,
        Matcher::Literal {
            case: Case::Insensitive,
            ..
        }
    ));
    assert!(insensitive.matches(Some("imdb.com")));
    assert!(insensitive.matches(Some("IMDB.COM")));
    assert!(!insensitive.matches(Some("www.imdb.com")));
    assert!(!insensitive.matches(None));

    let sensitive = Pattern::compile(Some("/Title"), Case::Sensitive);

    assert!(matches!(
        &sensitive.matcher,
        Matcher::Literal {
            case: Case::Sensitive,
            ..
        }
    ));
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

    assert!(matches!(
        &suffix.matcher,
        Matcher::Affix {
            case: Case::Insensitive,
            ..
        }
    ));
    assert!(suffix.matches(Some("mybot")));
    assert!(suffix.matches(Some("bot")));
    assert!(suffix.matches(Some("MYBOT")));
    assert!(!suffix.matches(Some("botx")));
    assert!(!suffix.matches(Some("mybotx")));
    assert!(!suffix.matches(Some("bo")));

    let prefix = Pattern::compile(Some("bot*"), Case::Sensitive);

    assert!(matches!(
        &prefix.matcher,
        Matcher::Affix {
            case: Case::Sensitive,
            ..
        }
    ));
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
    use std::time::Instant;

    #[allow(clippy::cast_precision_loss)] // iteration counts are far below 2^52
    fn measure(mut operation: impl FnMut() -> bool) -> f64 {
        const ITERATIONS: usize = 100_000;
        const RUNS: usize = 5;

        let mut best = f64::MAX;

        for _ in 0..RUNS {
            let start = Instant::now();

            for _ in 0..ITERATIONS {
                std::hint::black_box(operation());
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
        let pattern = std::hint::black_box(&pattern);
        let value = std::hint::black_box(value);
        let nanos = measure(|| pattern.matches(Some(value)));

        println!("affix benchmark: {label}: {value:?}: {nanos:.1} ns/op");
    }
}
