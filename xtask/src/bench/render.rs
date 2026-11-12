//! Markdown and Mermaid rendering of the benchmark report.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::bench::config;
use crate::bench::format::{natural_cmp, number, percent, signed};
use crate::bench::model::{Benchmark, Inventory, RunSummary, Tier};
use crate::bench::policy::{self, Verdict, counts, suite_totals};

/// First line of the headline, used as the check-run title.
#[must_use]
pub fn title(run: &RunSummary) -> String {
    headline(run)
        .lines()
        .next()
        .unwrap_or("Bench report")
        .to_owned()
}

#[must_use]
pub fn headline(run: &RunSummary) -> String {
    let verdict = policy::verdict(run);
    let counts = counts(&run.benchmarks);
    let mut parts = Vec::new();
    for (tier, label) in [
        (Tier::Regressed, "regression(s)"),
        (Tier::Warning, "warning(s)"),
        (Tier::Waived, "waived"),
        (Tier::Drift, "drift"),
        (Tier::Improved, "improved"),
        (Tier::Unchanged, "unchanged"),
        (Tier::New, "new"),
    ] {
        let count = counts.get(&tier).copied().unwrap_or(0);
        if count > 0 {
            parts.push(format!("{count} {label}"));
        }
    }
    let detail = if parts.is_empty() {
        "no benchmarks".to_owned()
    } else {
        parts.join(" · ")
    };

    match verdict {
        Verdict::Regressed => format!("Regressions detected — check fails\n\n{detail}"),
        Verdict::Warning => {
            format!("Toolchain mismatch — regressions reported as warnings\n\n{detail}")
        }
        Verdict::NoBaseline => {
            "No baseline recorded yet — this run records measurements but does not gate".to_owned()
        }
        Verdict::Error => "Benchmark run failed — see the job log".to_owned(),
        Verdict::Ok => format!("No regressions\n\n{detail}"),
    }
}

#[must_use]
pub fn metadata_block(run: &RunSummary) -> Vec<String> {
    if !run.baseline_found {
        return vec!["No last-known-good baseline artifact was available.".to_owned()];
    }

    let mut rows = Vec::new();
    for (key, label) in [
        ("rustc-release", "rustc"),
        ("valgrind", "valgrind"),
        ("gungraun-runner", "gungraun"),
    ] {
        rows.push(format!("{label} {}", or_unknown(run.current.get(key))));
    }
    let lock = if !run.baseline.get("cargo-lock-hash").is_empty()
        && run.baseline.get("cargo-lock-hash") == run.current.get("cargo-lock-hash")
    {
        "identical"
    } else {
        "changed"
    };

    let sha = run.baseline.get("commit");
    let sha = &sha[..sha.len().min(7)];
    let age = run.baseline.age_days().map_or_else(
        || "unknown age".to_owned(),
        |days| {
            if days == 0 {
                "today".to_owned()
            } else {
                format!("{days}d old")
            }
        },
    );
    let stale = run
        .baseline
        .age_days()
        .is_some_and(|days| days > config::REPORT.stale_days);

    let mut lines = vec![
        format!("{} · Cargo.lock: {lock}", rows.join(" · ")),
        format!(
            "Baseline: main@{sha} ({age}){}",
            if stale {
                format!(" — older than {} days", config::REPORT.stale_days)
            } else {
                String::new()
            }
        ),
    ];

    if !run.toolchain_matched {
        lines.extend([
            String::new(),
            "> **Toolchain mismatch vs baseline** — instruction counts are not comparable; \
             regressions are reported as warnings. Re-record the baseline from main \
             (`Benchmarks` → Run workflow) after the toolchain settles."
                .to_owned(),
            String::new(),
        ]);
        for mismatch in &run.mismatches {
            lines.push(format!(
                "> - `{}`: baseline `{}` vs this run `{}`",
                mismatch.key, mismatch.baseline, mismatch.current
            ));
        }
    }
    lines
}

const fn or_unknown(value: &str) -> &str {
    if value.is_empty() { "?" } else { value }
}

#[must_use]
pub fn build_report(run: &RunSummary) -> String {
    let mut lines = vec![
        "## Bench report".to_owned(),
        String::new(),
        headline(run),
        String::new(),
    ];
    lines.extend(metadata_block(run));
    lines.push(String::new());

    if run.baseline_found {
        lines.extend(totals_section(run));
        lines.extend(regression_impact_section(run));
        lines.extend(changed_sections(run));
        lines.extend(collapsed_sections(run));
        lines.extend(all_benchmarks_section(run));
    }

    lines.extend([
        "---".to_owned(),
        String::new(),
        format!(
            "*Methodology: one-shot Valgrind Callgrind and DHAT — counts are exact integers, \
             not statistical samples, so there are no confidence intervals. The chart shows \
             absolute `Ir` increases for regressions and marks the {} instruction floor; the \
             workflow gates on any reproducible increase above that floor. \
             Cross-implementation ratios are informational and never gated. Flamegraph SVGs and \
             the raw `summary.json` files are attached to the run artifacts.*",
            config::POLICY.floor
        ),
        String::new(),
    ]);
    lines.join("\n")
}

fn totals_section(run: &RunSummary) -> Vec<String> {
    let totals = suite_totals(&run.benchmarks);
    if totals.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    for (suite, (old, new)) in totals {
        #[allow(clippy::cast_precision_loss)]
        let pct = if old == 0 {
            0.0
        } else {
            (new as f64 - old as f64) / old as f64 * 100.0
        };
        parts.push(format!(
            "`{}` {} total Ir",
            config::suite_display(&suite),
            percent(Some(pct))
        ));
    }
    vec![
        format!("**Suite totals (orientation only):** {}", parts.join(" · ")),
        String::new(),
    ]
}

fn changed_sections(run: &RunSummary) -> Vec<String> {
    let mut lines = Vec::new();
    for (tier, title) in [
        (Tier::Regressed, "Regressions"),
        (Tier::Waived, "Waived regressions"),
        (Tier::Warning, "Warnings"),
    ] {
        let mut selected: Vec<&Benchmark> = run
            .benchmarks
            .iter()
            .filter(|benchmark| benchmark.tier == tier)
            .collect();
        selected
            .sort_by_key(|benchmark| std::cmp::Reverse(benchmark.ir.delta().unwrap_or(0).abs()));
        let (selected, extra) = cap(&selected, config::REPORT.max_table_rows);
        if selected.is_empty() {
            continue;
        }
        lines.push(format!("### {title}"));
        lines.push(String::new());
        lines.extend(table(&selected));
        lines.push(String::new());
        if extra > 0 {
            lines.push(format!(
                "*… and {extra} more; see the `bench-results` artifact.*"
            ));
            lines.push(String::new());
        }
    }
    lines
}

/// The regression impact chart: absolute `Ir` deltas against the gate floor.
fn regression_impact_section(run: &RunSummary) -> Vec<String> {
    let regressions = ir_regressions(&run.benchmarks);
    if regressions.is_empty() {
        return Vec::new();
    }

    let mut lines = vec![
        "### Regression impact".to_owned(),
        String::new(),
        format!(
            "Bars show the absolute `Ir` increase; the line marks the {} instruction floor.",
            config::POLICY.floor
        ),
        String::new(),
    ];
    lines.extend(impact_chart(&regressions));
    lines.push(String::new());
    lines.extend(impact_legend(&regressions));
    lines.push(String::new());
    lines
}

/// Positive `Ir` regressions, largest absolute increase first.
fn ir_regressions(benchmarks: &[Benchmark]) -> Vec<&Benchmark> {
    let mut regressions: Vec<&Benchmark> = benchmarks
        .iter()
        .filter(|benchmark| benchmark.tier == Tier::Regressed)
        .filter(|benchmark| benchmark.ir.delta().is_some_and(|delta| delta > 0))
        .collect();
    regressions.sort_by_key(|benchmark| std::cmp::Reverse(benchmark.ir.delta().unwrap_or(0)));
    regressions.truncate(config::REPORT.max_chart_bars);
    regressions
}

/// A bar chart of absolute `Ir` regressions with the policy floor as a line.
#[allow(clippy::cast_precision_loss)]
fn impact_chart(regressions: &[&Benchmark]) -> Vec<String> {
    let labels: Vec<String> = (1..=regressions.len())
        .map(|index| index.to_string())
        .collect();
    let values: Vec<u64> = regressions
        .iter()
        .map(|benchmark| benchmark.ir.delta().unwrap_or(0).unsigned_abs())
        .collect();
    let max = values.iter().copied().max().unwrap_or(0);
    let floor = config::POLICY.floor as f64;
    let best = (max as f64 * 1.1).max(floor);
    let line = |value: f64| {
        format!(
            "    line [{}]",
            labels
                .iter()
                .map(|_| format!("{value:.0}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    vec![
        "```mermaid".to_owned(),
        "---".to_owned(),
        "config:".to_owned(),
        "  themeVariables:".to_owned(),
        "    xyChart:".to_owned(),
        "      plotColorPalette: \"#cf222e, #d0d7de\"".to_owned(),
        "---".to_owned(),
        "xychart-beta".to_owned(),
        "    title \"Ir regression impact\"".to_owned(),
        format!(
            "    x-axis [{}]",
            labels
                .iter()
                .map(|label| format!("\"{label}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        format!("    y-axis \"Δ Ir\" 0 --> {best:.0}"),
        format!(
            "    bar [{}]",
            values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        line(floor),
        "```".to_owned(),
    ]
}

/// The numbered rows shared by the impact chart.
fn impact_legend(regressions: &[&Benchmark]) -> Vec<String> {
    let mut lines = vec![
        "**Chart legend**".to_owned(),
        String::new(),
        "| # | Benchmark | Δ Ir | Δ Ir% |".to_owned(),
        "| ---: | --- | ---: | ---: |".to_owned(),
    ];
    for (index, benchmark) in regressions.iter().enumerate() {
        let delta = benchmark.ir.delta().unwrap_or(0);
        lines.push(format!(
            "| {} | {} | {} | {} |",
            index + 1,
            benchmark.label(),
            number(Some(u64::try_from(delta).unwrap_or(0))),
            percent(benchmark.ir.pct)
        ));
    }
    lines
}

fn collapsed_sections(run: &RunSummary) -> Vec<String> {
    let mut lines = Vec::new();
    for (tier, title) in [
        (Tier::Drift, "Drift within limits"),
        (Tier::Improved, "Improvements"),
    ] {
        let selected: Vec<&Benchmark> = run
            .benchmarks
            .iter()
            .filter(|benchmark| benchmark.tier == tier)
            .collect();
        let (selected, extra) = cap(&selected, config::REPORT.max_table_rows);
        if selected.is_empty() {
            continue;
        }
        let mut body = table(&selected);
        if extra > 0 {
            body.push(format!("*… and {extra} more.*"));
            body.push(String::new());
        }
        lines.extend(details(
            &format!("{title} ({})", selected.len() + extra),
            body,
        ));
    }

    let comparisons = cross_implementation(&run.benchmarks);
    if !comparisons.is_empty() {
        lines.extend(details("Cross-implementation ratios", comparisons));
    }
    lines
}

fn all_benchmarks_section(run: &RunSummary) -> Vec<String> {
    let all: Vec<&Benchmark> = run.benchmarks.iter().collect();
    let (selected, extra) = cap(&all, config::REPORT.max_table_rows);
    let mut body = Vec::new();
    let suites: BTreeSet<&str> = selected
        .iter()
        .map(|benchmark| benchmark.suite.as_str())
        .collect();
    for suite in suites {
        body.push(format!("**{}**", config::suite_display(suite)));
        body.push(String::new());
        let benchmarks: Vec<&Benchmark> = selected
            .iter()
            .copied()
            .filter(|benchmark| benchmark.suite == suite)
            .collect();
        body.extend(table(&benchmarks));
        body.push(String::new());
    }
    if extra > 0 {
        body.push(format!(
            "*… and {extra} more; the complete list is in the `bench-results` artifact.*"
        ));
        body.push(String::new());
    }
    details(&format!("All benchmarks ({})", run.benchmarks.len()), body)
}

fn details(title: &str, body: Vec<String>) -> Vec<String> {
    let mut lines = vec![
        "<details>".to_owned(),
        format!("<summary>{title}</summary>"),
        String::new(),
    ];
    lines.extend(body);
    lines.push("</details>".to_owned());
    lines.push(String::new());
    lines
}

fn cap<'a>(benchmarks: &[&'a Benchmark], limit: usize) -> (Vec<&'a Benchmark>, usize) {
    if benchmarks.len() <= limit {
        return (benchmarks.to_vec(), 0);
    }
    (benchmarks[..limit].to_vec(), benchmarks.len() - limit)
}

#[must_use]
pub fn table(benchmarks: &[&Benchmark]) -> Vec<String> {
    let include_allocations = benchmarks
        .iter()
        .any(|benchmark| benchmark.has_allocations());
    let mut header = "| Benchmark | Case | Ir baseline → PR | Δ Ir | Δ Ir% | Verdict |".to_owned();
    let mut divider = "| --- | --- | ---: | ---: | ---: | --- |".to_owned();
    if include_allocations {
        header.push_str(" Alloc blocks | Alloc bytes |");
        divider.push_str(" ---: | ---: |");
    }

    let mut lines = vec![header, divider];
    for benchmark in benchmarks {
        let mut verdict = benchmark.tier.label().to_owned();
        if let Some(waiver) = &benchmark.waiver {
            let _ = write!(verdict, " — {waiver}");
        }
        let mut row = format!(
            "| {} | {} | {} → {} | {} | {} | {verdict}",
            config::display_name(&benchmark.function),
            benchmark
                .case
                .as_deref()
                .map_or_else(|| "—".to_owned(), config::humanize_case),
            number(benchmark.ir.old),
            number(benchmark.ir.new),
            signed(benchmark.ir.delta()),
            percent(benchmark.ir.pct)
        );
        if include_allocations {
            let _ = write!(
                row,
                " | {} → {} | {} → {}",
                number(benchmark.dhat_blocks.old),
                number(benchmark.dhat_blocks.new),
                number(benchmark.dhat_bytes.old),
                number(benchmark.dhat_bytes.new)
            );
        }
        row.push_str(" |");
        lines.push(row);
    }
    lines
}

#[allow(clippy::cast_precision_loss)]
fn cross_implementation(benchmarks: &[Benchmark]) -> Vec<String> {
    let inventory = Inventory::build(benchmarks);
    let mut lines = Vec::new();
    for (suite, reference) in config::COMPARISONS {
        let Some(reference_cases) = inventory.reference_cases(suite, reference) else {
            continue;
        };
        let mut members = inventory.comparison_members(suite, reference);
        members.sort_by_key(|member| config::display_name(member));
        if members.is_empty() {
            continue;
        }

        let suite_benchmarks: Vec<&Benchmark> = benchmarks
            .iter()
            .filter(|benchmark| benchmark.suite == *suite)
            .collect();
        let reference_ir: BTreeMap<&str, u64> = suite_benchmarks
            .iter()
            .filter(|benchmark| benchmark.function == *reference)
            .filter_map(|benchmark| Some((benchmark.case.as_deref()?, benchmark.ir.new?)))
            .collect();
        let mut cases: Vec<&str> = reference_cases
            .iter()
            .map(String::as_str)
            .filter(|case| reference_ir.contains_key(case))
            .collect();
        cases.sort_by(|left, right| natural_cmp(left, right));
        if cases.is_empty() {
            continue;
        }

        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!(
            "**{} vs {}**",
            capitalize(&config::suite_display(suite)),
            config::display_name(reference)
        ));
        lines.push(String::new());
        lines.push(format!(
            "| Case | {} (Ir) | {} |",
            config::display_name(reference),
            members
                .iter()
                .map(|member| config::display_name(member))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
        lines.push(format!("| --- | ---: |{}", " ---: |".repeat(members.len())));

        let by_key: BTreeMap<(&str, &str), u64> = suite_benchmarks
            .iter()
            .filter_map(|benchmark| {
                Some((
                    (benchmark.function.as_str(), benchmark.case.as_deref()?),
                    benchmark.ir.new?,
                ))
            })
            .collect();
        for case in cases {
            let base = reference_ir[case];
            let mut cells = vec![number(Some(base))];
            for member in &members {
                let value = by_key.get(&(member.as_str(), case));
                cells.push(match value {
                    None => "—".to_owned(),
                    Some(value) if base > 0 => format!(
                        "{} ({:.2}×)",
                        number(Some(*value)),
                        *value as f64 / base as f64
                    ),
                    Some(value) => number(Some(*value)),
                });
            }
            lines.push(format!(
                "| {} | {} |",
                config::humanize_case(case),
                cells.join(" | ")
            ));
        }
    }
    lines
}

fn capitalize(value: &str) -> String {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    first.to_uppercase().collect::<String>() + characters.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::metadata::Metadata;
    use crate::bench::model::Metrics;
    use crate::bench::policy::classify;

    #[allow(clippy::cast_precision_loss)]
    fn benchmark(function: &str, case: &str, old: u64, new: u64, suite: &str) -> Benchmark {
        let pct = if old == 0 {
            f64::INFINITY
        } else {
            (new as f64 - old as f64) / old as f64 * 100.0
        };
        Benchmark {
            suite: suite.to_owned(),
            function: function.to_owned(),
            case: Some(case.to_owned()),
            ir: Metrics {
                new: Some(new),
                old: Some(old),
                pct: Some(pct),
            },
            dhat_blocks: Metrics::default(),
            dhat_bytes: Metrics::default(),
            tier: Tier::Unchanged,
            reasons: Vec::new(),
            waiver: None,
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn renders_regressions_and_comparisons() {
        let mut regression = benchmark("bench_join_slice", "one_item", 100, 130, "join");
        let reference = benchmark("bench_std_join", "one_item", 100, 100, "join");
        let unchanged = benchmark("bench_distance_chrono", "leap_day", 200, 200, "duration");
        classify(&mut regression, &[]);
        let mut other = reference;
        classify(&mut other, &[]);
        let mut duration = unchanged;
        classify(&mut duration, &[]);

        let metadata = Metadata::default();
        let run = RunSummary {
            benchmarks: vec![regression, other, duration],
            baseline: metadata,
            current: Metadata::default(),
            baseline_found: true,
            toolchain_matched: true,
            mismatches: Vec::new(),
            exit_code: 3,
        };
        let report = build_report(&run);
        assert!(report.contains("Regressions detected"));
        assert!(report.contains("### Regressions"));
        assert!(report.contains("### Regression impact"));
        assert!(report.contains("Join vs std join (baseline)"));
        assert!(report.contains("xychart-beta"));
        assert!(report.contains("x-axis [\"1\"]"));
        assert!(report.contains("**Chart legend**"));
        assert!(report.contains("| 1 |"));
        assert!(!report.contains("pie showData"));
        assert!(!report.contains("quadrantChart"));
        assert!(report.contains("All benchmarks (3)"));
    }

    #[test]
    fn omits_the_regression_chart_when_nothing_regressed() {
        let mut unchanged = benchmark("bench_join_slice", "one_item", 100, 100, "join");
        classify(&mut unchanged, &[]);
        let run = RunSummary {
            benchmarks: vec![unchanged],
            baseline: Metadata::default(),
            current: Metadata::default(),
            baseline_found: true,
            toolchain_matched: true,
            mismatches: Vec::new(),
            exit_code: 0,
        };
        let report = build_report(&run);
        assert!(report.contains("No regressions"));
        assert!(!report.contains("Regression impact"));
        assert!(!report.contains("xychart-beta"));
        assert!(!report.contains("**Chart legend**"));
        assert!(!report.contains("pie showData"));
        assert!(!report.contains("quadrantChart"));
    }
}
