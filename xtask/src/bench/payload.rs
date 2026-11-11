//! The machine-readable verdict payload shared by the gate and publishing tasks.
//!
//! The document is the contract between `bench report` and the workflow steps that gate,
//! inspect and publish the result, so its field names and types are covered by a test.

use crate::bench::config;
use crate::bench::model::{RunSummary, Tier};
use crate::bench::policy;
use crate::bench::render;
use crate::json::{Object, Value};

/// Build the verdict document for `run`.
#[must_use]
pub fn verdict(run: &RunSummary) -> Value {
    let verdict = policy::verdict(run);
    let counts = policy::counts(&run.benchmarks);
    let count_map: Object = Tier::ALL
        .iter()
        .map(|tier| {
            (
                tier_name(*tier).to_owned(),
                Value::from(counts.get(tier).copied().unwrap_or(0)),
            )
        })
        .collect();

    let statuses: Object = policy::statuses(run)
        .into_iter()
        .map(|(context, status)| {
            (
                context,
                Value::object([
                    ("state", Value::from(status.state)),
                    ("description", Value::from(status.description)),
                ]),
            )
        })
        .collect();

    let regressions: Vec<Value> = run
        .benchmarks
        .iter()
        .filter(|benchmark| benchmark.tier == Tier::Regressed)
        .map(|benchmark| {
            Value::object([
                ("suite", Value::from(benchmark.suite.as_str())),
                ("benchmark", Value::from(benchmark.function.as_str())),
                ("case", benchmark.case.as_deref().into()),
                (
                    "display",
                    Value::from(config::label(
                        &benchmark.function,
                        benchmark.case.as_deref(),
                    )),
                ),
                (
                    "source",
                    Value::from(config::suite_source(&benchmark.suite)),
                ),
                ("ir_old", benchmark.ir.old.into()),
                ("ir_new", benchmark.ir.new.into()),
                (
                    "reasons",
                    Value::array(
                        benchmark
                            .reasons
                            .iter()
                            .map(|reason| Value::from(reason.as_str())),
                    ),
                ),
            ])
        })
        .collect();

    Value::object([
        ("verdict", Value::from(verdict.name())),
        ("conclusion", Value::from(verdict.conclusion())),
        ("title", Value::from(render::title(run))),
        ("counts", Value::Object(count_map)),
        ("statuses", Value::Object(statuses)),
        ("regressions", Value::Array(regressions)),
    ])
}

const fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Regressed => "regressed",
        Tier::Warning => "warning",
        Tier::Waived => "waived",
        Tier::Drift => "drift",
        Tier::Improved => "improved",
        Tier::Unchanged => "unchanged",
        Tier::New => "new",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::metadata::Metadata;
    use crate::bench::model::{Benchmark, Metrics};
    use crate::bench::policy::classify;
    use crate::json;

    #[test]
    fn verdict_payload_keeps_the_schema() {
        let mut benchmark = Benchmark {
            suite: "join".to_owned(),
            function: "bench_x".to_owned(),
            case: None,
            ir: Metrics {
                new: Some(120),
                old: Some(100),
                pct: Some(20.0),
            },
            dhat_blocks: Metrics::default(),
            dhat_bytes: Metrics::default(),
            tier: Tier::Unchanged,
            reasons: Vec::new(),
            waiver: None,
        };
        classify(&mut benchmark, &[]);
        let run = RunSummary {
            benchmarks: vec![benchmark],
            baseline: Metadata::default(),
            current: Metadata::default(),
            baseline_found: true,
            toolchain_matched: true,
            mismatches: Vec::new(),
            exit_code: 0,
        };

        let payload = verdict(&run);
        assert_eq!(
            payload.get("verdict").and_then(Value::as_str),
            Some("regressed")
        );
        assert_eq!(
            payload.get("conclusion").and_then(Value::as_str),
            Some("failure")
        );
        let counts = payload
            .get("counts")
            .and_then(Value::as_object)
            .expect("counts object");
        for tier in [
            "regressed",
            "warning",
            "waived",
            "drift",
            "improved",
            "unchanged",
            "new",
        ] {
            assert!(counts.contains_key(tier), "missing count `{tier}`");
        }
        assert_eq!(counts["regressed"].as_u64(), Some(1));

        let regression = &payload
            .get("regressions")
            .and_then(Value::as_array)
            .expect("regressions array")[0];
        for key in [
            "suite",
            "benchmark",
            "case",
            "display",
            "source",
            "ir_old",
            "ir_new",
            "reasons",
        ] {
            assert!(regression.get(key).is_some(), "missing field `{key}`");
        }
        assert_eq!(regression.get("case"), Some(&Value::Null));
        assert_eq!(regression.get("ir_old").and_then(Value::as_u64), Some(100));
        assert_eq!(regression.get("ir_new").and_then(Value::as_u64), Some(120));
        assert_eq!(
            regression.get("source").and_then(Value::as_str),
            Some("benchmarks/benches/join.rs")
        );

        let status = payload
            .get("statuses")
            .and_then(|statuses| statuses.get("bench/join"))
            .expect("join status");
        assert_eq!(status.get("state").and_then(Value::as_str), Some("failure"));
        assert_eq!(
            status.get("description").and_then(Value::as_str),
            Some("1 regression(s) vs main")
        );

        let serialized = json::to_string(&payload);
        assert_eq!(
            json::parse(&serialized).ok().as_ref(),
            Some(&payload),
            "verdict must round-trip through the serializer"
        );
    }
}
