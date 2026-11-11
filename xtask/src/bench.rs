//! Benchmark-related xtask commands.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::args::Options;
use crate::check::{self, CheckResult};
use crate::config;
use crate::error::{Error, Result};
use crate::metadata::{self, Metadata};
use crate::model::{self, Tier};
use crate::policy::{self, RunSummary};
use crate::publish::{self, PublishOptions};
use crate::render;

/// Dispatch `cargo xtask bench <command>`.
pub fn run(arguments: &[String]) -> Result<()> {
    let Some((command, rest)) = arguments.split_first() else {
        print_usage();
        return Err(Error::new("expected a benchmark task"));
    };
    match command.as_str() {
        "metadata" => metadata(rest),
        "check-config" => check_config(rest),
        "report" => report(rest),
        "gate" => gate(rest),
        "has-regressions" => has_regressions(rest),
        "post-report" => publish(rest),
        "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        other => Err(Error::new(format!("unknown benchmark task `{other}`"))),
    }
}

fn print_usage() {
    println!(
        "Usage: cargo xtask bench <command>

Commands:
  metadata      Record the benchmark environment metadata
                --output <FILE> [--toolchain <NAME>]
  check-config  Validate the report configuration against the measured benchmarks
                [--summaries <DIR>]
  report        Render the benchmark report and verdict from Gungraun summaries
                [--summaries <DIR>] [--baseline-metadata <FILE>]
                [--current-metadata <FILE>] [--baseline-found]
                [--bench-exit-code <N>] [--report <FILE>] [--verdict <FILE>]
  gate          Fail (exit 1) unless the verdict is passing
                [--verdict <FILE>]
  has-regressions  Print true when the verdict contains regressions
                [--verdict <FILE>]
  post-report   Post the verdict as a check run and commit statuses
                [--verdict <FILE>] [--report <FILE>] [--dry-run]"
    );
}

fn metadata(arguments: &[String]) -> Result<()> {
    let options = Options::parse(arguments, &["output", "toolchain"], &[])?;
    let output = options.path_or("output", "bench-metadata.txt");
    let toolchain = options.value("toolchain").unwrap_or("stable");
    metadata::record(&output, toolchain)
}

fn check_config(arguments: &[String]) -> Result<()> {
    let options = Options::parse(arguments, &["summaries"], &[])?;
    let summaries = options.path_or("summaries", "target/gungraun");
    let benchmarks = model::load(&summaries)?;
    let result: CheckResult = check::check(&benchmarks, config::WAIVERS);
    println!("{}", check::format_result(&result, "xtask/src/config.rs"));
    if result.ok() {
        Ok(())
    } else {
        Err(Error::new("configuration drift detected"))
    }
}

fn report(arguments: &[String]) -> Result<()> {
    let options = Options::parse(
        arguments,
        &[
            "summaries",
            "baseline-metadata",
            "current-metadata",
            "bench-exit-code",
            "report",
            "verdict",
        ],
        &["baseline-found"],
    )?;
    let summaries = options.path_or("summaries", "target/gungraun");
    let baseline_metadata =
        options.path_or("baseline-metadata", "target/gungraun/bench-metadata.txt");
    let current_metadata = options.path_or("current-metadata", "bench-metadata.txt");
    let report_path = options.path_or("report", "bench-report.md");
    let verdict_path = options.path_or("verdict", "verdict.json");

    let baseline = Metadata::read(&baseline_metadata)?;
    let current = Metadata::read(&current_metadata)?;
    let mismatches = baseline.mismatches(&current);
    let baseline_found = options.flag("baseline-found");
    let exit_code = options.value("bench-exit-code").map_or(Ok(0), |value| {
        value
            .parse::<i32>()
            .map_err(|_| Error::new(format!("invalid --bench-exit-code `{value}`")))
    })?;

    let mut benchmarks = model::load(&summaries)?;
    for benchmark in &mut benchmarks {
        policy::classify(benchmark, config::WAIVERS);
        // Counts recorded with a different toolchain are not comparable; keep reporting
        // them, but never gate on them.
        if !mismatches.is_empty() && benchmark.tier == Tier::Regressed {
            benchmark.tier = Tier::Warning;
            benchmark.waiver = Some("toolchain mismatch".to_owned());
        }
    }

    let run = RunSummary {
        benchmarks,
        baseline,
        current,
        baseline_found,
        toolchain_matched: mismatches.is_empty() || !baseline_found,
        mismatches,
        exit_code,
    };

    write_text(&report_path, &render::build_report(&run))?;
    write_json(&verdict_path, &verdict_payload(&run))?;
    println!("{}", policy::verdict(&run).name());
    Ok(())
}

fn gate(arguments: &[String]) -> Result<()> {
    let options = Options::parse(arguments, &["verdict"], &[])?;
    let verdict = read_verdict(&options.path_or("verdict", "verdict.json"))?;
    match verdict["verdict"].as_str().unwrap_or("error") {
        "regressed" | "error" => {
            eprintln!("::error::Benchmark regressions were detected; see the Bench report check.");
            Err(Error::new("benchmark gate failed"))
        }
        _ => Ok(()),
    }
}

fn has_regressions(arguments: &[String]) -> Result<()> {
    let options = Options::parse(arguments, &["verdict"], &[])?;
    let verdict = read_verdict(&options.path_or("verdict", "verdict.json"))?;
    let regressed = verdict["counts"]["regressed"].as_u64().unwrap_or(0);
    println!("{}", regressed > 0);
    Ok(())
}

fn read_verdict(path: &Path) -> Result<Value> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn publish(arguments: &[String]) -> Result<()> {
    let options = Options::parse(arguments, &["verdict", "report"], &["dry-run"])?;
    let publish_options = PublishOptions {
        verdict: options.path_or("verdict", "verdict.json"),
        report: options.path_or("report", "bench-report.md"),
        dry_run: options.flag("dry-run"),
    };
    publish::prepare(&publish_options)?;
    publish::publish(&publish_options)
}

#[must_use]
fn verdict_payload(run: &RunSummary) -> Value {
    let verdict = policy::verdict(run);
    let counts = policy::counts(&run.benchmarks);
    let count_map: Map<String, Value> = Tier::ALL
        .iter()
        .map(|tier| {
            (
                tier_name(*tier).to_owned(),
                Value::from(counts.get(tier).copied().unwrap_or(0)),
            )
        })
        .collect();

    let statuses: Map<String, Value> = policy::statuses(run)
        .into_iter()
        .map(|(context, status)| {
            (
                context,
                json!({"state": status.state, "description": status.description}),
            )
        })
        .collect();

    let regressions: Vec<Value> = run
        .benchmarks
        .iter()
        .filter(|benchmark| benchmark.tier == Tier::Regressed)
        .map(|benchmark| {
            json!({
                "suite": benchmark.suite,
                "benchmark": benchmark.function,
                "case": benchmark.case,
                "display": config::label(&benchmark.function, benchmark.case.as_deref()),
                "source": config::suite_source(&benchmark.suite),
                "ir_old": benchmark.ir.old,
                "ir_new": benchmark.ir.new,
                "reasons": benchmark.reasons,
            })
        })
        .collect();

    json!({
        "verdict": verdict.name(),
        "conclusion": verdict.conclusion(),
        "title": render::title(run),
        "counts": count_map,
        "statuses": statuses,
        "regressions": regressions,
    })
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

fn write_text(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    write_text(path, &format!("{}\n", serde_json::to_string_pretty(value)?))
}
