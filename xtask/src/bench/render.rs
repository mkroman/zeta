//! Markdown and Mermaid rendering of the benchmark report.
//!
//! The pull-request report is deliberately small: the verdict, a metadata line and a table of
//! only the benchmarks that changed. The complete inventory and the static
//! cross-implementation ratios stay available behind [`ReportOptions::full`] (the baseline job
//! uses it); the raw summaries in the run artifact remain the authoritative data.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::bench::config;
use crate::bench::format::{natural_cmp, number, percent, signed};
use crate::bench::model::{Benchmark, Inventory, Metrics, RunSummary, Tier};
use crate::bench::policy::{self, Verdict, counts, suite_totals};

/// How much of the report to render.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReportOptions {
    /// Keep the complete inventory (instead of per-suite counts) and the static
    /// cross-implementation ratios. The baseline job renders the full report; pull-request
    /// summaries stay concise.
    pub full: bool,
}

/// Short verdict phrase, used as the check-run title.
#[must_use]
pub fn title(run: &RunSummary) -> &'static str {
    verdict_title(policy::verdict(run))
}

const fn verdict_title(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Ok => "No regressions",
        Verdict::Regressed => "Regressions detected",
        Verdict::Warning => "Toolchain mismatch",
        Verdict::NoBaseline => "No baseline recorded yet",
        Verdict::Error => "Benchmark run failed",
    }
}

/// One-line verdict and counts, without the table's repeated detail.
#[must_use]
pub fn headline(run: &RunSummary) -> String {
    let verdict = policy::verdict(run);
    let counts = counts(&run.benchmarks);
    let detail = tier_summary(&counts);
    let unchanged = counts.get(&Tier::Unchanged).copied().unwrap_or(0);
    let changed = run.benchmarks.len().saturating_sub(unchanged);
    match verdict {
        Verdict::Ok | Verdict::Warning if changed == 0 => {
            format!(
                "{} — 0 changed, {unchanged} unchanged",
                verdict_title(verdict)
            )
        }
        Verdict::Ok | Verdict::Regressed | Verdict::Warning => {
            format!("{} — {detail}", verdict_title(verdict))
        }
        Verdict::NoBaseline => format!(
            "{} — nothing to compare, this run only records measurements",
            verdict_title(verdict)
        ),
        Verdict::Error => format!("{} — see the job log", verdict_title(verdict)),
    }
}

/// `1 regression · 2 improvements · 72 unchanged`, empty tiers omitted.
fn tier_summary(counts: &BTreeMap<Tier, usize>) -> String {
    let mut parts = Vec::new();
    for tier in [
        Tier::Regressed,
        Tier::Warning,
        Tier::Waived,
        Tier::Drift,
        Tier::Improved,
        Tier::New,
        Tier::Unchanged,
    ] {
        let count = counts.get(&tier).copied().unwrap_or(0);
        if count > 0 {
            parts.push(format!("{count} {}", tier_label(tier, count)));
        }
    }
    if parts.is_empty() {
        "no benchmarks".to_owned()
    } else {
        parts.join(" · ")
    }
}

const fn tier_label(tier: Tier, count: usize) -> &'static str {
    match tier {
        Tier::Regressed if count == 1 => "regression",
        Tier::Regressed => "regressions",
        Tier::Warning if count == 1 => "warning",
        Tier::Warning => "warnings",
        Tier::Waived if count == 1 => "waived regression",
        Tier::Waived => "waived regressions",
        Tier::Drift => "drift",
        Tier::Improved if count == 1 => "improvement",
        Tier::Improved => "improvements",
        Tier::New => "new",
        Tier::Unchanged => "unchanged",
    }
}

/// The single metadata line: baseline identity, toolchain and `Cargo.lock` state.
#[must_use]
pub fn metadata_line(run: &RunSummary) -> String {
    if !run.baseline_found {
        return "No last-known-good baseline artifact — counts recorded, nothing to compare."
            .to_owned();
    }

    let mut parts = Vec::new();
    for (key, label) in [
        ("rustc-release", "rustc"),
        ("valgrind", "valgrind"),
        ("gungraun-runner", "gungraun"),
    ] {
        let value = or_unknown(run.current.get(key));
        parts.push(format!("{label} {}", tool_version(key, value)));
    }

    let lock = if !run.baseline.get("cargo-lock-hash").is_empty()
        && run.baseline.get("cargo-lock-hash") == run.current.get("cargo-lock-hash")
    {
        "identical"
    } else {
        "changed"
    };
    parts.push(format!("Cargo.lock {lock}"));

    let sha = run.baseline.get("commit");
    let sha = if sha.is_empty() {
        "?"
    } else {
        &sha[..sha.len().min(7)]
    };
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
    parts.push(format!(
        "baseline main@{sha} ({age}{})",
        if stale {
            format!(", older than {} days", config::REPORT.stale_days)
        } else {
            String::new()
        }
    ));

    parts.join(" · ")
}

/// One compact line when the baseline was recorded with a different toolchain.
#[must_use]
pub fn mismatch_line(run: &RunSummary) -> Option<String> {
    if run.toolchain_matched {
        return None;
    }
    let details = run
        .mismatches
        .iter()
        .map(|mismatch| {
            format!(
                "`{}` {} → {}",
                mismatch.key, mismatch.baseline, mismatch.current
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "> **Toolchain mismatch** — {details}. Counts are not comparable, so regressions are \
         reported as warnings; re-record the `main` baseline after the toolchain settles."
    ))
}

const fn or_unknown(value: &str) -> &str {
    if value.is_empty() { "?" } else { value }
}

/// Tool versions often repeat their own name (`valgrind-3.27.1`); drop that redundant prefix
/// so the metadata line reads `valgrind 3.27.1` instead of `valgrind valgrind-3.27.1`.
fn tool_version<'a>(key: &str, value: &'a str) -> &'a str {
    match key {
        "valgrind" => value.strip_prefix("valgrind-").unwrap_or(value),
        "gungraun-runner" => value.strip_prefix("gungraun-runner ").unwrap_or(value),
        _ => value,
    }
}

/// Build the Markdown report for `run`.
#[must_use]
pub fn build_report(run: &RunSummary, options: ReportOptions) -> String {
    let mut lines = vec![
        "## Bench report".to_owned(),
        String::new(),
        headline(run),
        String::new(),
        metadata_line(run),
    ];
    if let Some(mismatch) = mismatch_line(run) {
        lines.push(mismatch);
    }
    lines.push(String::new());

    if run.baseline_found {
        if let Some(totals) = totals_line(run) {
            lines.push(totals);
            lines.push(String::new());
        }

        let changed = changed_table(run);
        if !changed.is_empty() {
            lines.extend(changed);
            lines.push(String::new());
        }

        if options.full {
            let impact = regression_impact_section(run);
            if !impact.is_empty() {
                lines.extend(impact);
            }
        }

        lines.extend(inventory_section(run, options.full));

        if options.full {
            lines.extend(orientation_section(run));
        }
    }

    lines.push("---".to_owned());
    lines.push(String::new());
    lines.push(methodology_line(options.full));
    lines.push(String::new());
    lines.join("\n")
}

/// Suite totals are orientation only, so they appear only when a suite total actually moved.
#[allow(clippy::cast_precision_loss)]
fn totals_line(run: &RunSummary) -> Option<String> {
    let mut parts = Vec::new();
    for (suite, (old, new)) in suite_totals(&run.benchmarks) {
        if old == 0 || new == old {
            continue;
        }
        let pct = (new as f64 - old as f64) / old as f64 * 100.0;
        if pct.abs() < 0.005 {
            continue;
        }
        parts.push(format!(
            "`{}` {} total Ir",
            config::suite_display(&suite),
            percent(Some(pct))
        ));
    }
    (!parts.is_empty()).then(|| format!("Suite totals: {}.", parts.join(" · ")))
}

/// The changed tiers in one table: regressions first, then warnings, waivers, drift,
/// improvements and new benchmarks, each group by descending `|Δ Ir%|`.
fn changed_table(run: &RunSummary) -> Vec<String> {
    let mut changed: Vec<&Benchmark> = run
        .benchmarks
        .iter()
        .filter(|benchmark| benchmark.tier != Tier::Unchanged)
        .collect();
    if changed.is_empty() {
        return Vec::new();
    }
    changed.sort_by(|left, right| compare_changed(left, right));

    let total = changed.len();
    let (selected, extra) = cap(&changed, config::REPORT.max_table_rows);
    let mut lines = table(&selected);
    if extra > 0 {
        lines.push(String::new());
        lines.push(format!(
            "*… and {extra} more of {total} changed; the complete list is in the artifact.*"
        ));
    }
    lines
}

fn compare_changed(left: &Benchmark, right: &Benchmark) -> Ordering {
    left.tier
        .cmp(&right.tier)
        .then_with(|| percent_magnitude(right).total_cmp(&percent_magnitude(left)))
        .then_with(|| left.suite.cmp(&right.suite))
        .then_with(|| left.function.cmp(&right.function))
        .then_with(|| left.case.cmp(&right.case))
}

fn percent_magnitude(benchmark: &Benchmark) -> f64 {
    benchmark
        .ir
        .pct
        .filter(|pct| pct.is_finite())
        .map_or(0.0, f64::abs)
}

/// The regression impact chart: absolute `Ir` deltas against the gate floor. Full reports only.
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

/// The inventory: per-suite counts by default, every row with [`ReportOptions::full`].
fn inventory_section(run: &RunSummary, full: bool) -> Vec<String> {
    if run.benchmarks.is_empty() {
        return Vec::new();
    }

    let mut body = Vec::new();
    if full {
        let all: Vec<&Benchmark> = run.benchmarks.iter().collect();
        let (selected, extra) = cap(&all, config::REPORT.max_table_rows);
        let suites: BTreeSet<&str> = selected
            .iter()
            .map(|benchmark| benchmark.suite.as_str())
            .collect();
        for (index, suite) in suites.into_iter().enumerate() {
            if index > 0 {
                body.push(String::new());
            }
            body.push(format!("**{}**", config::suite_display(suite)));
            body.push(String::new());
            let benchmarks: Vec<&Benchmark> = selected
                .iter()
                .copied()
                .filter(|benchmark| benchmark.suite == suite)
                .collect();
            body.extend(table(&benchmarks));
        }
        if extra > 0 {
            body.push(String::new());
            body.push(format!(
                "*… and {extra} more; the complete list is in the `bench-results` artifact.*"
            ));
        }
    } else {
        let mut suites: Vec<&str> = run
            .benchmarks
            .iter()
            .map(|benchmark| benchmark.suite.as_str())
            .collect();
        suites.sort_unstable();
        suites.dedup();
        for suite in suites {
            let mut suite_counts = BTreeMap::new();
            for benchmark in run
                .benchmarks
                .iter()
                .filter(|benchmark| benchmark.suite == suite)
            {
                *suite_counts.entry(benchmark.tier).or_insert(0) += 1;
            }
            body.push(format!(
                "**{}** — {}",
                config::suite_display(suite),
                tier_summary(&suite_counts)
            ));
        }
    }

    details(&format!("Full inventory ({})", run.benchmarks.len()), body)
}

/// Static orientation data: cross-implementation ratios are not PR deltas.
fn orientation_section(run: &RunSummary) -> Vec<String> {
    let comparisons = cross_implementation(&run.benchmarks);
    if comparisons.is_empty() {
        Vec::new()
    } else {
        details("Cross-implementation ratios", comparisons)
    }
}

fn details(title: &str, mut body: Vec<String>) -> Vec<String> {
    let mut lines = vec![
        "<details>".to_owned(),
        format!("<summary>{title}</summary>"),
        String::new(),
    ];
    if !body.last().is_some_and(String::is_empty) {
        body.push(String::new());
    }
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

/// One delta-focused table. Allocation deltas appear only when at least one row changed an
/// allocation count; unchanged columns are otherwise dropped.
#[must_use]
pub fn table(benchmarks: &[&Benchmark]) -> Vec<String> {
    let include_allocations = benchmarks.iter().any(|benchmark| {
        benchmark
            .dhat_blocks
            .delta()
            .is_some_and(|delta| delta != 0)
            || benchmark.dhat_bytes.delta().is_some_and(|delta| delta != 0)
    });

    let mut header = "| Benchmark | Case | Ir | Δ Ir | Δ Ir% | Verdict |".to_owned();
    let mut divider = "| --- | --- | ---: | ---: | ---: | --- |".to_owned();
    if include_allocations {
        header.push_str(" Δ Blocks | Δ Bytes |");
        divider.push_str(" ---: | ---: |");
    }

    let mut lines = vec![header, divider];
    for benchmark in benchmarks {
        let mut verdict = benchmark.tier.label().to_owned();
        if let Some(waiver) = &benchmark.waiver {
            let _ = write!(verdict, " — {waiver}");
        }
        let mut row = format!(
            "| {} | {} | {} | {} | {} | {verdict}",
            config::display_name(&benchmark.function),
            benchmark
                .case
                .as_deref()
                .map_or_else(|| "—".to_owned(), config::humanize_case),
            number(benchmark.ir.new),
            signed(benchmark.ir.delta()),
            percent(benchmark.ir.pct)
        );
        if include_allocations {
            let _ = write!(
                row,
                " | {} | {}",
                allocation_delta(&benchmark.dhat_blocks),
                allocation_delta(&benchmark.dhat_bytes)
            );
        }
        row.push_str(" |");
        lines.push(row);
    }
    lines
}

/// Allocation columns only carry signal when the count moved; untouched rows stay blank.
fn allocation_delta(metrics: &Metrics) -> String {
    match metrics.delta() {
        Some(0) | None => "—".to_owned(),
        Some(delta) => signed(Some(delta)),
    }
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

fn methodology_line(full: bool) -> String {
    let ratios = if full {
        " Cross-implementation ratios are informational and never gated."
    } else {
        ""
    };
    format!(
        "*Methodology: one-shot Valgrind Callgrind and DHAT — exact instruction counts, not \
         samples. The workflow gates on any reproducible `Ir` increase above the {} instruction \
         floor.{ratios} Full summaries and flamegraphs are attached to the run artifacts.*",
        config::POLICY.floor
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::metadata::{Metadata, Mismatch};
    use crate::bench::model::Metrics;
    use crate::bench::policy::classify;

    #[allow(clippy::cast_precision_loss)]
    fn benchmark(
        function: &str,
        case: &str,
        old: Option<u64>,
        new: Option<u64>,
        suite: &str,
    ) -> Benchmark {
        let pct = match (old, new) {
            (Some(old), Some(new)) if old > 0 => {
                Some((new as f64 - old as f64) / old as f64 * 100.0)
            }
            _ => None,
        };
        Benchmark {
            suite: suite.to_owned(),
            function: function.to_owned(),
            case: Some(case.to_owned()),
            ir: Metrics { new, old, pct },
            dhat_blocks: Metrics::default(),
            dhat_bytes: Metrics::default(),
            tier: Tier::Unchanged,
            reasons: Vec::new(),
            waiver: None,
        }
    }

    fn summary(benchmarks: Vec<Benchmark>) -> RunSummary {
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

    fn render(mut benchmarks: Vec<Benchmark>, options: ReportOptions) -> String {
        for benchmark in &mut benchmarks {
            classify(benchmark, &[]);
        }
        build_report(&summary(benchmarks), options)
    }

    #[test]
    fn concise_no_change_report_is_short_and_signed_correctly() {
        let report = render(
            vec![
                benchmark(
                    "bench_oxford_join_slice",
                    "one_item",
                    Some(100),
                    Some(100),
                    "join",
                ),
                benchmark(
                    "bench_oxford_join_slice",
                    "two_items",
                    Some(200),
                    Some(200),
                    "join",
                ),
            ],
            ReportOptions::default(),
        );
        assert!(
            report.lines().count() <= 20,
            "no-change report is not concise:\n{report}"
        );
        assert!(report.contains("No regressions — 0 changed, 2 unchanged"));
        assert!(report.contains("**join** — 2 unchanged"));
        assert!(!report.contains("-0"));
        assert!(!report.contains("+0.00%"));
        assert!(!report.contains("Cross-implementation ratios"));
        assert!(!report.contains("xychart-beta"));
        assert!(!report.contains("baseline → PR"));
    }

    #[test]
    fn concise_report_renders_one_regression() {
        let report = render(
            vec![
                benchmark(
                    "bench_join_with_separators",
                    "eight_items",
                    Some(1_000),
                    Some(1_300),
                    "join",
                ),
                benchmark(
                    "bench_std_join",
                    "eight_items",
                    Some(500),
                    Some(500),
                    "join",
                ),
            ],
            ReportOptions::default(),
        );
        assert!(report.contains("Regressions detected — 1 regression · 1 unchanged"));
        assert!(report.contains("| Benchmark | Case | Ir | Δ Ir | Δ Ir% | Verdict |"));
        assert!(report.contains(
            "| join with separators | eight items | 1,300 | +300 | +30.00% | regression |"
        ));
        assert!(report.contains("**join** — 1 regression · 1 unchanged"));
        assert!(!report.contains("Δ Blocks"));
        assert!(!report.contains("Cross-implementation ratios"));
        assert!(!report.contains("xychart-beta"));
    }

    #[test]
    fn concise_report_renders_one_improvement() {
        let report = render(
            vec![benchmark(
                "bench_join_with_separators",
                "eight_items",
                Some(1_000),
                Some(900),
                "join",
            )],
            ReportOptions::default(),
        );
        assert!(report.contains("No regressions — 1 improvement"));
        assert!(
            report.contains(
                "| join with separators | eight items | 900 | -100 | -10.00% | improved |"
            )
        );
        assert!(report.contains("**join** — 1 improvement"));
        assert!(!report.contains("xychart-beta"));
    }

    #[test]
    fn concise_report_renders_a_new_benchmark() {
        let report = render(
            vec![benchmark(
                "bench_brand_new",
                "one_item",
                None,
                Some(42),
                "join",
            )],
            ReportOptions::default(),
        );
        assert!(report.contains("No regressions — 1 new"));
        assert!(report.contains("| brand new | one item | 42 | — | — | new |"));
        assert!(report.contains("**join** — 1 new"));
    }

    #[test]
    fn concise_report_explains_a_toolchain_mismatch() {
        let mut warning = benchmark("bench_x", "case_0", Some(1_000), Some(1_100), "join");
        classify(&mut warning, &[]);
        warning.tier = Tier::Warning;
        warning.waiver = Some("toolchain mismatch".to_owned());
        let mut run = summary(vec![warning]);
        run.toolchain_matched = false;
        run.exit_code = 3;
        run.mismatches = vec![Mismatch {
            key: "rustc-release".to_owned(),
            baseline: "1.99.0".to_owned(),
            current: "1.100.0".to_owned(),
        }];

        let report = build_report(&run, ReportOptions::default());
        assert!(report.contains("Toolchain mismatch — 1 warning"));
        assert!(report.contains(
            "> **Toolchain mismatch** — `rustc-release` 1.99.0 → 1.100.0. Counts are not \
             comparable"
        ));
        assert!(report.contains("| warning — toolchain mismatch |"));
        assert!(!report.contains("Cross-implementation ratios"));

        let mut clean = summary(vec![benchmark(
            "bench_y",
            "case_0",
            Some(500),
            Some(500),
            "duration",
        )]);
        clean.toolchain_matched = false;
        clean.mismatches = vec![Mismatch {
            key: "rustc-release".to_owned(),
            baseline: "1.99.0".to_owned(),
            current: "1.100.0".to_owned(),
        }];
        assert!(
            build_report(&clean, ReportOptions::default())
                .contains("Toolchain mismatch — 0 changed, 1 unchanged")
        );
    }

    #[test]
    fn full_report_keeps_the_inventory_and_orientation_tables() {
        let mut regression = benchmark(
            "bench_oxford_join_slice",
            "one_item",
            Some(100),
            Some(130),
            "join",
        );
        let mut reference = benchmark("bench_std_join", "one_item", Some(100), Some(100), "join");
        let mut unchanged = benchmark(
            "bench_distance_in_words_chrono",
            "leap_day",
            Some(200),
            Some(200),
            "duration",
        );
        classify(&mut regression, &[]);
        classify(&mut reference, &[]);
        classify(&mut unchanged, &[]);
        let run = summary(vec![regression, reference, unchanged]);

        let report = build_report(&run, ReportOptions { full: true });
        assert!(report.contains("### Regression impact"));
        assert!(report.contains("xychart-beta"));
        assert!(report.contains("Cross-implementation ratios"));
        assert!(report.contains("Join vs std join (baseline)"));
        assert!(report.contains("Full inventory (3)"));
        assert!(
            report.contains("| std join (baseline) | one item | 100 | 0 | 0.00% | unchanged |")
        );
        assert!(
            report.contains(
                "| distance_in_words (chrono) | leap day | 200 | 0 | 0.00% | unchanged |"
            )
        );
        assert!(!report.contains("-0"));
        assert!(!report.contains("+0.00%"));

        let concise = build_report(&run, ReportOptions::default());
        assert!(!concise.contains("Cross-implementation ratios"));
        assert!(!concise.contains("xychart-beta"));
        assert!(!concise.contains("| std join (baseline) |"));
        assert!(concise.contains("**join** — 1 regression · 1 unchanged"));
    }

    #[test]
    fn allocation_columns_appear_only_when_an_allocation_changed() {
        let mut changed = benchmark("bench_x", "case_0", Some(100), Some(100), "join");
        changed.dhat_blocks = Metrics {
            new: Some(2),
            old: Some(1),
            pct: None,
        };
        changed.dhat_bytes = Metrics {
            new: Some(64),
            old: Some(64),
            pct: None,
        };
        let report = render(vec![changed], ReportOptions::default());
        assert!(report.contains("| Δ Blocks | Δ Bytes |"));
        assert!(report.contains("| 100 | 0 | 0.00% | regression | +1 | — |"));

        let unchanged = render(
            vec![benchmark("bench_x", "case_0", Some(100), Some(130), "join")],
            ReportOptions::default(),
        );
        assert!(!unchanged.contains("Δ Blocks"));
    }
}
