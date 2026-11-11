//! Regression policy: verdict tiers, waivers and per-suite status contexts.

use std::collections::BTreeMap;

use crate::config::{self, Metric, Waiver};
use crate::format::{percent, signed};
use crate::metadata::{Metadata, Mismatch};
use crate::model::{Benchmark, Tier};

/// One run's inputs for policy decisions and rendering.
pub struct RunSummary {
    pub benchmarks: Vec<Benchmark>,
    pub baseline: Metadata,
    pub current: Metadata,
    pub baseline_found: bool,
    pub toolchain_matched: bool,
    pub mismatches: Vec<Mismatch>,
    pub exit_code: i32,
}

/// The overall verdict for a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Regressed,
    Warning,
    NoBaseline,
    Error,
}

impl Verdict {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Regressed => "regressed",
            Self::Warning => "warning",
            Self::NoBaseline => "no-baseline",
            Self::Error => "error",
        }
    }

    /// Checks API conclusion.
    #[must_use]
    pub const fn conclusion(self) -> &'static str {
        match self {
            Self::Ok => "success",
            Self::Regressed | Self::Error => "failure",
            Self::Warning | Self::NoBaseline => "neutral",
        }
    }
}

/// One commit status per suite.
pub struct Status {
    pub state: String,
    pub description: String,
}

/// Count benchmarks per tier.
#[must_use]
pub fn counts(benchmarks: &[Benchmark]) -> BTreeMap<Tier, usize> {
    let mut counts = BTreeMap::new();
    for benchmark in benchmarks {
        *counts.entry(benchmark.tier).or_insert(0) += 1;
    }
    counts
}

/// Sum new and old `Ir` counts per suite (orientation only, never gated).
#[must_use]
pub fn suite_totals(benchmarks: &[Benchmark]) -> BTreeMap<String, (u64, u64)> {
    let mut totals = BTreeMap::<String, (u64, u64)>::new();
    for benchmark in benchmarks {
        let (Some(old), Some(new)) = (benchmark.ir.old, benchmark.ir.new) else {
            continue;
        };
        let entry = totals.entry(benchmark.suite.clone()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(old);
        entry.1 = entry.1.saturating_add(new);
    }
    totals
}

/// Assign a verdict tier and the human-readable reasons to `benchmark`.
pub fn classify(benchmark: &mut Benchmark, waivers: &[Waiver]) {
    let policy = &config::POLICY;

    if benchmark.ir.old.is_none() {
        benchmark.tier = Tier::New;
        return;
    }

    let deltas = [
        (Metric::Ir, benchmark.ir.delta()),
        (Metric::Blocks, benchmark.dhat_blocks.delta()),
        (Metric::Bytes, benchmark.dhat_bytes.delta()),
    ];

    let mut breached = Vec::new();
    for (metric, delta) in deltas {
        let Some(delta) = delta else {
            continue;
        };
        if delta <= 0 {
            continue;
        }
        match metric {
            Metric::Ir => {
                let pct = benchmark.ir.pct;
                let floor = i64::try_from(policy.floor).unwrap_or(i64::MAX);
                let over = delta >= floor && pct.is_none_or(|pct| pct > policy.limit);
                if over {
                    breached.push(metric);
                    let detail = pct.map_or_else(
                        || format!("limit {:.1}%", policy.limit),
                        |pct| format!("{}, limit {:.1}%", percent(Some(pct)), policy.limit),
                    );
                    benchmark
                        .reasons
                        .push(format!("Ir {} ({detail})", signed(Some(delta))));
                } else {
                    benchmark.reasons.push(format!(
                        "Ir {} (below {} instruction floor)",
                        signed(Some(delta)),
                        policy.floor
                    ));
                }
            }
            Metric::Blocks => {
                breached.push(metric);
                benchmark
                    .reasons
                    .push(format!("allocations {} block(s)", signed(Some(delta))));
            }
            Metric::Bytes => {
                breached.push(metric);
                benchmark
                    .reasons
                    .push(format!("allocated bytes {}", signed(Some(delta))));
            }
        }
    }

    if breached.is_empty() {
        benchmark.tier = if deltas.iter().any(|(_, delta)| delta.is_some_and(|d| d < 0)) {
            Tier::Improved
        } else if deltas.iter().any(|(_, delta)| delta.is_some_and(|d| d > 0)) {
            Tier::Drift
        } else {
            Tier::Unchanged
        };
        return;
    }

    let mut waived = Vec::new();
    for metric in &breached {
        if let Some(waiver) = waivers
            .iter()
            .find(|waiver| waiver.matches(&benchmark.function, benchmark.case.as_deref(), *metric))
        {
            waived.push((*metric, waiver));
        }
    }
    if waived.len() == breached.len() {
        benchmark.tier = Tier::Waived;
        benchmark.waiver = Some(
            waived
                .iter()
                .map(|(metric, waiver)| format!("{}: {}", metric.name(), waiver.describe()))
                .collect::<Vec<_>>()
                .join("; "),
        );
    } else {
        benchmark.tier = Tier::Regressed;
    }
}

/// Decide the overall verdict for a run.
#[must_use]
pub fn verdict(run: &RunSummary) -> Verdict {
    let counts = counts(&run.benchmarks);
    if run.exit_code != 0 && run.exit_code != 3 {
        return Verdict::Error;
    }
    if !run.baseline_found {
        return Verdict::NoBaseline;
    }
    if counts.get(&Tier::Regressed).copied().unwrap_or(0) > 0 {
        return Verdict::Regressed;
    }
    if !run.toolchain_matched {
        return Verdict::Warning;
    }
    Verdict::Ok
}

/// Build one commit status per dynamically discovered suite.
#[must_use]
pub fn statuses(run: &RunSummary) -> BTreeMap<String, Status> {
    let verdict = verdict(run);
    let mut payload = BTreeMap::new();

    let mut suites: Vec<&str> = run
        .benchmarks
        .iter()
        .map(|benchmark| benchmark.suite.as_str())
        .collect();
    suites.sort_unstable();
    suites.dedup();

    for suite in suites {
        let benchmarks: Vec<&Benchmark> = run
            .benchmarks
            .iter()
            .filter(|benchmark| benchmark.suite == suite)
            .collect();
        let regressed = benchmarks
            .iter()
            .filter(|benchmark| benchmark.tier == Tier::Regressed)
            .count();
        let waived = benchmarks
            .iter()
            .filter(|benchmark| benchmark.tier == Tier::Waived)
            .count();
        let total = benchmarks
            .iter()
            .filter(|benchmark| benchmark.ir.old.is_some())
            .count();

        let description = if !run.baseline_found {
            "no baseline recorded yet".to_owned()
        } else if !run.toolchain_matched {
            "warning: toolchain mismatch".to_owned()
        } else if regressed > 0 {
            format!("{regressed} regression(s) vs main")
        } else if waived > 0 {
            format!("{waived} waived vs main")
        } else {
            format!("{total}/{total} within limits vs main")
        };

        let state = if verdict == Verdict::Error || regressed > 0 {
            "failure"
        } else {
            "success"
        };
        payload.insert(
            config::suite_context(suite),
            Status {
                state: state.to_owned(),
                description,
            },
        );
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Waiver;
    use crate::model::Metrics;

    #[allow(clippy::cast_precision_loss)]
    fn benchmark(function: &str, old: Option<u64>, new: Option<u64>, suite: &str) -> Benchmark {
        let pct = match (old, new) {
            (Some(old), Some(new)) if old > 0 => {
                Some((new as f64 - old as f64) / old as f64 * 100.0)
            }
            _ => None,
        };
        Benchmark {
            suite: suite.to_owned(),
            function: function.to_owned(),
            case: Some("case_0".to_owned()),
            ir: Metrics { new, old, pct },
            dhat_blocks: Metrics::default(),
            dhat_bytes: Metrics::default(),
            tier: Tier::Unchanged,
            reasons: Vec::new(),
            waiver: None,
        }
    }

    fn run(benchmarks: Vec<Benchmark>) -> RunSummary {
        RunSummary {
            benchmarks,
            baseline: Metadata::default(),
            current: Metadata::default(),
            baseline_found: true,
            toolchain_matched: true,
            mismatches: Vec::new(),
            exit_code: 0,
        }
    }

    #[test]
    fn classifies_tiers() {
        let mut regression = benchmark("bench_x", Some(1000), Some(1030), "join");
        let mut drift = benchmark("bench_x", Some(1000), Some(1010), "join");
        let mut improvement = benchmark("bench_x", Some(1000), Some(900), "join");
        let mut new = benchmark("bench_x", None, Some(100), "join");
        for item in [&mut regression, &mut drift, &mut improvement, &mut new] {
            classify(item, &[]);
        }
        assert_eq!(regression.tier, Tier::Regressed);
        assert_eq!(drift.tier, Tier::Drift);
        assert_eq!(improvement.tier, Tier::Improved);
        assert_eq!(new.tier, Tier::New);
    }

    #[test]
    fn applies_waivers() {
        let mut regression = benchmark("bench_x", Some(1000), Some(1030), "join");
        let waiver = Waiver {
            benchmark: "bench_x",
            case: Some("case_0"),
            metric: Metric::Ir,
            reason: "accepted",
            expires: None,
        };
        classify(&mut regression, &[waiver]);
        assert_eq!(regression.tier, Tier::Waived);
    }

    #[test]
    fn reports_per_suite_statuses() {
        let mut regression = benchmark("bench_x", Some(1000), Some(1030), "join");
        classify(&mut regression, &[]);
        let mut ok = benchmark("bench_y", Some(500), Some(500), "duration");
        classify(&mut ok, &[]);
        let statuses = statuses(&run(vec![regression, ok]));
        assert_eq!(statuses["bench/join"].state, "failure");
        assert_eq!(statuses["bench/duration"].state, "success");
    }
}
