//! Configuration hygiene checks against the measured benchmark inventory.
//!
//! `cargo xtask bench check-config` runs this after a benchmark run (the summaries are the
//! authoritative inventory) and fails when the Rust constants in `bench/config.rs` or the
//! accepted waivers reference something that no longer exists. That keeps renames and
//! removals from silently disabling display names, comparison groups or waivers.

use std::collections::{BTreeMap, BTreeSet};

use crate::bench::config::{self, Metric, Waiver};
use crate::bench::model::{Benchmark, Inventory};

pub struct CheckResult {
    pub benchmarks: usize,
    pub suites: usize,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl CheckResult {
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn check(benchmarks: &[Benchmark], waivers: &[Waiver]) -> CheckResult {
    let inventory = Inventory::build(benchmarks);
    let mut result = CheckResult {
        benchmarks: benchmarks.len(),
        suites: inventory.suites.len(),
        errors: Vec::new(),
        warnings: Vec::new(),
    };

    for (function, display) in config::DISPLAY {
        if !inventory.cases_by_function.contains_key(*function) {
            result.errors.push(format!(
                "[display] {function}: no such benchmark (renamed or removed)"
            ));
        } else if *display == config::humanize(function) {
            result.warnings.push(format!(
                "[display] {function}: override \"{display}\" is the derived name"
            ));
        }
    }

    for suite in config::SUITES {
        if !inventory.suites.contains(suite.name) {
            result
                .errors
                .push(format!("[suites] {}: no such suite", suite.name));
        }
    }

    for (suite, reference) in config::COMPARISONS {
        if !inventory.suites.contains(*suite) {
            result
                .errors
                .push(format!("[comparisons].{suite}: no such suite"));
            continue;
        }
        let Some(reference_cases) = inventory.reference_cases(suite, reference) else {
            result.errors.push(format!(
                "[comparisons].{suite}: reference {reference} is not a benchmark in the suite"
            ));
            continue;
        };
        if reference_cases.is_empty() {
            result.errors.push(format!(
                "[comparisons].{suite}: reference {reference} has no cases"
            ));
            continue;
        }
        if inventory.comparison_members(suite, reference).is_empty() {
            result.errors.push(format!(
                "[comparisons].{suite}: no benchmark shares the reference's case-id set, \
                 so the comparison would render nothing"
            ));
        }
    }

    let mut duplicates = BTreeMap::<(String, Option<String>, Metric), usize>::new();
    for waiver in waivers {
        let key = (
            waiver.benchmark.to_owned(),
            waiver.case.map(str::to_owned),
            waiver.metric,
        );
        *duplicates.entry(key).or_insert(0) += 1;
    }
    let mut reported_duplicates = BTreeSet::new();
    for waiver in waivers {
        let key = (
            waiver.benchmark.to_owned(),
            waiver.case.map(str::to_owned),
            waiver.metric,
        );
        if duplicates.get(&key).copied().unwrap_or(0) > 1 && reported_duplicates.insert(key) {
            result.warnings.push(format!(
                "waiver for {}{} ({}) is duplicated",
                waiver.benchmark,
                waiver
                    .case
                    .map_or_else(String::new, |case| format!("·{case}")),
                waiver.metric.name()
            ));
        }
        if !inventory.cases_by_function.contains_key(waiver.benchmark) {
            result.errors.push(format!(
                "waiver for {}: no such benchmark (renamed or removed)",
                waiver.benchmark
            ));
            continue;
        }
        if let Some(case) = waiver.case
            && !inventory.cases_by_function[waiver.benchmark].contains(case)
        {
            result.errors.push(format!(
                "waiver for {}·{case}: no such case id",
                waiver.benchmark
            ));
        }
        if waiver.expired() {
            result.warnings.push(format!(
                "waiver for {}: expired {} — remove it or extend it with a fresh justification",
                waiver.benchmark,
                waiver.expires.unwrap_or("")
            ));
        }
    }

    result
}

#[must_use]
pub fn format_result(result: &CheckResult, source: &str) -> String {
    let suite_word = if result.suites == 1 {
        "suite"
    } else {
        "suites"
    };
    let mut lines = vec![format!(
        "Config check: {} benchmarks across {} {suite_word} (`{source}`)",
        result.benchmarks, result.suites
    )];
    if result.ok() && result.warnings.is_empty() {
        lines.push("No configuration drift.".to_owned());
    }
    for error in &result.errors {
        lines.push(format!("error: {error}"));
    }
    for warning in &result.warnings {
        lines.push(format!("warning: {warning}"));
    }
    if !result.ok() {
        lines.push(
            "The configuration references benchmarks that no longer exist; update \
             `xtask/src/bench/config.rs`."
                .to_owned(),
        );
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::model::{Metrics, Tier};

    fn benchmark(function: &str, case: &str, suite: &str) -> Benchmark {
        Benchmark {
            suite: suite.to_owned(),
            function: function.to_owned(),
            case: Some(case.to_owned()),
            ir: Metrics::default(),
            dhat_blocks: Metrics::default(),
            dhat_bytes: Metrics::default(),
            tier: Tier::Unchanged,
            reasons: Vec::new(),
            waiver: None,
        }
    }

    fn inventory() -> Vec<Benchmark> {
        vec![
            benchmark("bench_std_join", "case_0", "join"),
            benchmark("bench_join_slice", "case_0", "join"),
            benchmark("bench_parse_iso8601", "case_0", "join"),
            benchmark("bench_format_seconds_full_table", "case_0", "join"),
            benchmark("bench_format_seconds_hours_and_minutes", "case_0", "join"),
            benchmark("bench_distance_in_words_chrono", "case_0", "duration"),
            benchmark("bench_distance_in_words_time", "case_0", "duration"),
            benchmark("bench_time_duration_in_words", "case_0", "duration"),
        ]
    }

    #[test]
    fn clean_configuration_passes() {
        let result = check(&inventory(), &[]);
        assert!(result.ok(), "{:?}", result.errors);
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    }

    #[test]
    fn warns_about_expired_waivers() {
        let waiver = Waiver {
            benchmark: "bench_std_join",
            case: None,
            metric: Metric::Ir,
            reason: "old",
            expires: Some("2020-01-01"),
        };
        let result = check(&inventory(), &[waiver]);
        assert!(result.ok());
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("expired"))
        );
    }

    #[test]
    fn detects_unmatched_waiver() {
        let waiver = Waiver {
            benchmark: "bench_removed",
            case: None,
            metric: Metric::Ir,
            reason: "gone",
            expires: None,
        };
        let result = check(&inventory(), &[waiver]);
        assert!(!result.ok());
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.contains("bench_removed"))
        );
    }
}
