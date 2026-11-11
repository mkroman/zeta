//! The measured benchmark model, loaded from Gungraun's typed v7 summaries, plus the
//! inventory indexes shared by the report, policy and configuration checks.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config;
use crate::error::{Error, Result};
use crate::json::{self, Value};

/// One metric's new value, baseline value and recorded percentage difference.
#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub new: Option<u64>,
    pub old: Option<u64>,
    pub pct: Option<f64>,
}

impl Metrics {
    #[must_use]
    pub fn delta(&self) -> Option<i64> {
        let new = i64::try_from(self.new?).ok()?;
        let old = i64::try_from(self.old?).ok()?;
        new.checked_sub(old)
    }
}

/// Verdict tier for one benchmark case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Regressed,
    Warning,
    Waived,
    Drift,
    Improved,
    Unchanged,
    New,
}

impl Tier {
    pub const ALL: [Self; 7] = [
        Self::Regressed,
        Self::Warning,
        Self::Waived,
        Self::Drift,
        Self::Improved,
        Self::Unchanged,
        Self::New,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Regressed => "🔴 regression",
            Self::Warning => "⚠ warning",
            Self::Waived => "⚠ waived",
            Self::Drift => "🟡 drift",
            Self::Improved => "🟢 improved",
            Self::Unchanged => "⚪ unchanged",
            Self::New => "🆕 new",
        }
    }
}

/// One measured benchmark case.
#[derive(Clone, Debug)]
pub struct Benchmark {
    pub suite: String,
    pub function: String,
    pub case: Option<String>,
    pub ir: Metrics,
    pub dhat_blocks: Metrics,
    pub dhat_bytes: Metrics,
    pub tier: Tier,
    pub reasons: Vec<String>,
    pub waiver: Option<String>,
}

impl Benchmark {
    #[must_use]
    pub fn label(&self) -> String {
        config::label(&self.function, self.case.as_deref())
    }

    #[must_use]
    pub const fn has_allocations(&self) -> bool {
        self.dhat_blocks.old.is_some()
            || self.dhat_blocks.new.is_some()
            || self.dhat_bytes.old.is_some()
            || self.dhat_bytes.new.is_some()
    }
}

/// Load every summary below `directory`.
pub fn load(directory: &Path) -> Result<Vec<Benchmark>> {
    let mut summaries = Vec::new();
    collect_summaries(directory, &mut summaries)?;
    summaries.sort();

    let mut benchmarks = Vec::new();
    for path in summaries {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("warning: skipping {}: {error}", path.display());
                continue;
            }
        };
        let summary = match json::parse(&text) {
            Ok(summary) => summary,
            Err(error) => {
                eprintln!("warning: skipping {}: {error}", path.display());
                continue;
            }
        };
        match benchmark_from_summary(&summary) {
            Some(benchmark) => benchmarks.push(benchmark),
            None => eprintln!(
                "warning: skipping {}: not a Callgrind summary",
                path.display()
            ),
        }
    }

    benchmarks.sort_by(|left, right| {
        (&left.suite, &left.function, &left.case).cmp(&(&right.suite, &right.function, &right.case))
    });
    Ok(benchmarks)
}

/// Extract the fields the report needs from a v7 summary document.
fn benchmark_from_summary(summary: &Value) -> Option<Benchmark> {
    let function = summary.get("function_name")?.as_str()?.to_owned();
    let case = summary.get("id").and_then(Value::as_str).map(str::to_owned);
    let suite = summary
        .get("benchmark_file")
        .and_then(Value::as_str)
        .and_then(|path| Path::new(path).file_stem())
        .map_or_else(
            || "unknown".to_owned(),
            |stem| stem.to_string_lossy().into_owned(),
        );

    let profiles = summary.get("profiles")?.as_array()?;
    let callgrind = profiles
        .iter()
        .find(|profile| profile.get("tool").and_then(Value::as_str) == Some("Callgrind"))?;
    let callgrind_metrics = callgrind.get("data")?.get("total")?.get("metrics")?;

    let dhat = profiles
        .iter()
        .find(|profile| profile.get("tool").and_then(Value::as_str) == Some("DHAT"))
        .and_then(|profile| profile.get("data")?.get("total")?.get("metrics"));
    let (dhat_blocks, dhat_bytes) = dhat.map_or_else(
        || (Metrics::default(), Metrics::default()),
        |metrics| {
            (
                metrics_of(metrics, "TotalBlocks"),
                metrics_of(metrics, "TotalBytes"),
            )
        },
    );

    Some(Benchmark {
        suite,
        function,
        case,
        ir: metrics_of(callgrind_metrics, "Ir"),
        dhat_blocks,
        dhat_bytes,
        tier: Tier::Unchanged,
        reasons: Vec::new(),
        waiver: None,
    })
}

fn collect_summaries(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(directory).map_err(|error| {
        Error::new(format!(
            "cannot read summaries in {}: {error}",
            directory.display()
        ))
    })?;
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            collect_summaries(&path, paths)?;
        } else if path.file_name().is_some_and(|name| name == "summary.json") {
            paths.push(path);
        }
    }
    Ok(())
}

fn metrics_of(metrics: &Value, key: &str) -> Metrics {
    let Some(entry) = metrics.get(key) else {
        return Metrics::default();
    };
    let values = entry.get("values");
    Metrics {
        new: values
            .and_then(|values| values.get("new"))
            .and_then(Value::as_u64),
        old: values
            .and_then(|values| values.get("old"))
            .and_then(Value::as_u64),
        pct: entry
            .get("change")
            .and_then(|change| change.get("diff_pct"))
            .and_then(Value::as_str)
            .and_then(parse_percent),
    }
}

/// Gungraun serializes `diff_pct` as a string so infinity survives JSON.
fn parse_percent(raw: &str) -> Option<f64> {
    match raw {
        "inf" | "+inf" => Some(f64::INFINITY),
        "-inf" => Some(f64::NEG_INFINITY),
        raw => raw.parse().ok(),
    }
}

/// Indexes over the measured benchmarks, shared by rendering and configuration checks.
#[derive(Debug, Default)]
pub struct Inventory {
    pub cases_by_function: BTreeMap<String, BTreeSet<String>>,
    pub functions_by_suite: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    pub suites: BTreeSet<String>,
}

impl Inventory {
    #[must_use]
    pub fn build(benchmarks: &[Benchmark]) -> Self {
        let mut inventory = Self::default();
        for benchmark in benchmarks {
            inventory.suites.insert(benchmark.suite.clone());
            inventory
                .cases_by_function
                .entry(benchmark.function.clone())
                .or_default();
            let functions = inventory
                .functions_by_suite
                .entry(benchmark.suite.clone())
                .or_default();
            functions.entry(benchmark.function.clone()).or_default();
            if let Some(case) = &benchmark.case {
                inventory
                    .cases_by_function
                    .entry(benchmark.function.clone())
                    .or_default()
                    .insert(case.clone());
                functions
                    .entry(benchmark.function.clone())
                    .or_default()
                    .insert(case.clone());
            }
        }
        inventory
    }

    #[must_use]
    pub fn reference_cases(&self, suite: &str, reference: &str) -> Option<&BTreeSet<String>> {
        self.functions_by_suite.get(suite)?.get(reference)
    }

    /// Functions in `suite` using exactly the reference's case-id set.
    #[must_use]
    pub fn comparison_members(&self, suite: &str, reference: &str) -> Vec<String> {
        let Some(functions) = self.functions_by_suite.get(suite) else {
            return Vec::new();
        };
        let Some(cases) = functions.get(reference) else {
            return Vec::new();
        };
        if cases.is_empty() {
            return Vec::new();
        }
        let mut members: Vec<String> = functions
            .iter()
            .filter(|(function, function_cases)| {
                function.as_str() != reference && *function_cases == cases
            })
            .map(|(function, _)| function.clone())
            .collect();
        members.sort();
        members
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn computes_metric_deltas() {
        let metrics = Metrics {
            new: Some(120),
            old: Some(100),
            pct: Some(20.0),
        };
        assert_eq!(metrics.delta(), Some(20));
        assert_eq!(Metrics::default().delta(), None);
    }

    #[test]
    fn extracts_benchmark_from_summary_json() {
        let summary = json::parse(
            r#"{
                "function_name": "bench_x",
                "id": "case_0",
                "benchmark_file": "benches/join.rs",
                "profiles": [
                    {
                        "tool": "Callgrind",
                        "data": { "total": { "metrics": {
                            "Ir": {
                                "values": { "new": 120, "old": 100 },
                                "change": { "diff_pct": "20.0", "factor": "1.2" }
                            }
                        } } }
                    },
                    {
                        "tool": "DHAT",
                        "data": { "total": { "metrics": {
                            "TotalBlocks": { "values": { "new": 2, "old": 1 } },
                            "TotalBytes": { "values": { "new": 64, "old": 32 } }
                        } } }
                    }
                ]
            }"#,
        )
        .expect("valid summary JSON");
        let benchmark = benchmark_from_summary(&summary).expect("callgrind summary");
        assert_eq!(benchmark.suite, "join");
        assert_eq!(benchmark.function, "bench_x");
        assert_eq!(benchmark.case.as_deref(), Some("case_0"));
        assert_eq!(benchmark.ir.new, Some(120));
        assert_eq!(benchmark.ir.old, Some(100));
        assert_eq!(benchmark.ir.pct, Some(20.0));
        assert_eq!(benchmark.dhat_blocks.new, Some(2));
        assert_eq!(benchmark.dhat_bytes.new, Some(64));
    }

    #[test]
    fn parses_infinite_percentages() {
        assert_eq!(parse_percent("inf"), Some(f64::INFINITY));
        assert_eq!(parse_percent("-inf"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_percent("12.5"), Some(12.5));
        assert_eq!(parse_percent("garbage"), None);
    }

    #[test]
    fn skips_summaries_without_callgrind() {
        let summary = json::parse(r#"{"function_name":"x","profiles":[]}"#).expect("valid JSON");
        assert!(benchmark_from_summary(&summary).is_none());
    }

    #[test]
    fn accepts_missing_or_null_case_ids() {
        for document in [
            r#"{"function_name":"x","id":null,"profiles":[{"tool":"Callgrind","data":{"total":{"metrics":{}}}}]}"#,
            r#"{"function_name":"x","profiles":[{"tool":"Callgrind","data":{"total":{"metrics":{}}}}]}"#,
        ] {
            let summary = json::parse(document).expect("valid JSON");
            let benchmark = benchmark_from_summary(&summary).expect("callgrind summary");
            assert_eq!(benchmark.case, None);
            assert_eq!(benchmark.suite, "unknown");
        }
    }

    #[test]
    fn inventory_finds_comparison_members_by_case_set() {
        let benchmarks = vec![
            benchmark("bench_std_join", "one_item", "join"),
            benchmark("bench_slice", "one_item", "join"),
            benchmark("bench_lengths", "item_length_0", "join"),
            benchmark("bench_chrono", "leap_day", "duration"),
            benchmark("bench_time", "leap_day", "duration"),
        ];
        let inventory = Inventory::build(&benchmarks);
        assert_eq!(inventory.suites.len(), 2);
        assert_eq!(
            inventory.comparison_members("join", "bench_std_join"),
            vec!["bench_slice".to_owned()]
        );
        assert_eq!(
            inventory.comparison_members("duration", "bench_chrono"),
            vec!["bench_time".to_owned()]
        );
        assert_eq!(
            inventory.comparison_members("join", "bench_lengths"),
            Vec::<String>::new()
        );
    }
}
