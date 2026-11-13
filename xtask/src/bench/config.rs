//! Benchmark report policy, presentation constants and accepted waivers.
//!
//! Keeping this configuration in Rust makes it compile-time checked: there are no keys to
//! mistype and no schema to validate. The command `bench check-config` still validates the
//! names against the measured benchmark inventory.

use crate::time;

/// Gating policy for exact, deterministic counts.
pub struct Policy {
    /// A positive `Ir` delta below this many instructions is churn on trivial benches.
    pub floor: u64,
    /// Exact counts with a matched toolchain gate on any reproducible increase.
    pub limit: f64,
}

pub const POLICY: Policy = Policy {
    floor: 16,
    limit: 0.0,
};

/// Presentation limits, separate from the gating policy.
pub struct ReportLimits {
    pub stale_days: i64,
    pub max_chart_bars: usize,
    pub max_table_rows: usize,
}

pub const REPORT: ReportLimits = ReportLimits {
    stale_days: 7,
    max_chart_bars: 12,
    max_table_rows: 120,
};

/// Everything needed to describe, annotate and link one benchmark suite.
pub struct Suite {
    pub name: &'static str,
    /// Source file regressions are annotated against.
    pub source: &'static str,
    /// Commit status context.
    pub context: &'static str,
}

pub const SUITES: &[Suite] = &[
    Suite {
        name: "join",
        source: "benchmarks/benches/join.rs",
        context: "bench/join",
    },
    Suite {
        name: "duration",
        source: "benchmarks/benches/duration.rs",
        context: "bench/duration",
    },
    Suite {
        name: "dispatch",
        source: "benchmarks/benches/dispatch.rs",
        context: "bench/dispatch",
    },
];

/// Optional display overrides; functions not listed are humanized from their Rust names.
pub const DISPLAY: &[(&str, &str)] = &[
    ("bench_std_join", "std join (baseline)"),
    ("bench_parse_iso8601", "parse_iso8601_duration"),
    (
        "bench_format_seconds_full_table",
        "format_seconds_in_words (full table)",
    ),
    (
        "bench_format_seconds_hours_and_minutes",
        "format_seconds_in_words (h/m)",
    ),
    (
        "bench_distance_in_words_chrono",
        "distance_in_words (chrono)",
    ),
    ("bench_distance_in_words_time", "distance_in_words (time)"),
    ("bench_time_duration_in_words", "duration::in_words"),
];

/// Within-run comparison groups: suite name and the reference benchmark. Other benchmarks in
/// the suite that use the same set of case ids are compared against the reference
/// automatically. Informational only, never gated.
pub const COMPARISONS: &[(&str, &str)] = &[
    ("join", "bench_std_join"),
    ("duration", "bench_distance_in_words_chrono"),
];

/// The metric a waiver applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Metric {
    Ir,
    Blocks,
    Bytes,
}

impl Metric {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ir => "Ir",
            Self::Blocks => "TotalBlocks",
            Self::Bytes => "TotalBytes",
        }
    }
}

/// An accepted regression, recorded with a reason and optional expiry.
pub struct Waiver {
    pub benchmark: &'static str,
    pub case: Option<&'static str>,
    pub metric: Metric,
    pub reason: &'static str,
    /// `YYYY-MM-DD`; the waiver stops applying afterwards.
    pub expires: Option<&'static str>,
}

/// Accepted regressions. Add an entry in the pull request that accepts the regression so the
/// trade-off is reviewed and stays visible in the report.
pub const WAIVERS: &[Waiver] = &[
    // Waiver {
    //     benchmark: "bench_oxford_join_filtered_iterator",
    //     case: Some("sixty_four_items"),
    //     metric: Metric::Ir,
    //     reason: "Keeps the zero-allocation contract; +4.5% Ir accepted for the API shape.",
    //     expires: Some("2027-05-01"),
    // },
];

impl Waiver {
    pub fn expired(&self) -> bool {
        self.expires
            .and_then(time::parse_date)
            .is_some_and(|expiry| expiry < time::today())
    }

    pub fn matches(&self, benchmark: &str, case: Option<&str>, metric: Metric) -> bool {
        self.benchmark == benchmark
            && self.metric == metric
            && self.case.is_none_or(|configured| Some(configured) == case)
            && !self.expired()
    }

    pub fn describe(&self) -> String {
        self.expires.map_or_else(
            || self.reason.to_owned(),
            |expires| format!("{} (expires {expires})", self.reason),
        )
    }
}

/// Turn a Rust benchmark function name into a readable label.
#[must_use]
pub fn humanize(function: &str) -> String {
    function.trim_prefix("bench_").replace('_', " ")
}

/// Turn a benchmark case id into a readable label.
#[must_use]
pub fn humanize_case(case: &str) -> String {
    case.replace('_', " ")
}

#[must_use]
pub fn display_name(function: &str) -> String {
    DISPLAY
        .iter()
        .find_map(|(name, display)| (*name == function).then(|| (*display).to_owned()))
        .unwrap_or_else(|| humanize(function))
}

#[must_use]
pub fn label(function: &str, case: Option<&str>) -> String {
    let name = display_name(function);
    match case {
        Some(case) => format!("{name} · {}", humanize_case(case)),
        None => name,
    }
}

#[must_use]
pub fn suite(name: &str) -> Option<&'static Suite> {
    SUITES.iter().find(|suite| suite.name == name)
}

#[must_use]
pub fn suite_display(name: &str) -> String {
    name.to_owned()
}

#[must_use]
pub fn suite_source(name: &str) -> String {
    suite(name).map_or_else(
        || format!("benchmarks/benches/{name}.rs"),
        |suite| suite.source.to_owned(),
    )
}

#[must_use]
pub fn suite_context(name: &str) -> String {
    suite(name).map_or_else(|| format!("bench/{name}"), |suite| suite.context.to_owned())
}
