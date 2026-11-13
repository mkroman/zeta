//! Benchmark tasks: metadata, configuration checks, reporting, gating and publishing.
//!
//! The tasks share the measured model and policy modules in this directory; the CLI layer in
//! [`crate::cli`] validates and types each task's arguments before dispatch.

mod check;
mod config;
mod format;
mod metadata;
mod model;
mod payload;
mod policy;
mod publish;
mod render;

use std::fs;
use std::path::Path;

use crate::bench::check::CheckResult;
use crate::bench::metadata::Metadata;
use crate::bench::model::{RunSummary, Tier};
use crate::bench::publish::PublishOptions;
use crate::cli::{
    BenchCommand, CheckConfigArgs, GateArgs, HasRegressionsArgs, MetadataArgs, PostReportArgs,
    ReportArgs,
};
use crate::error::{Error, Result};
use crate::json::{self, Value};

/// Run one parsed benchmark task.
pub fn run(command: BenchCommand) -> Result<()> {
    match command {
        BenchCommand::Metadata(args) => metadata(&args),
        BenchCommand::CheckConfig(args) => check_config(&args),
        BenchCommand::Report(args) => report(&args),
        BenchCommand::Gate(args) => gate(&args),
        BenchCommand::HasRegressions(args) => has_regressions(&args),
        BenchCommand::PostReport(args) => post_report(args),
    }
}

fn metadata(args: &MetadataArgs) -> Result<()> {
    metadata::record(&args.output, &args.toolchain)
}

fn check_config(args: &CheckConfigArgs) -> Result<()> {
    let benchmarks = model::load(&args.summaries)?;
    let result: CheckResult = check::check(&benchmarks, config::WAIVERS);
    println!(
        "{}",
        check::format_result(&result, "xtask/src/bench/config.rs")
    );
    if result.ok() {
        Ok(())
    } else {
        Err(Error::new("configuration drift detected"))
    }
}

fn report(args: &ReportArgs) -> Result<()> {
    let baseline = Metadata::read(&args.baseline_metadata)?;
    let current = Metadata::read(&args.current_metadata)?;
    let mismatches = baseline.mismatches(&current);

    let mut benchmarks = model::load(&args.summaries)?;
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
        baseline_found: args.baseline_found,
        toolchain_matched: mismatches.is_empty() || !args.baseline_found,
        mismatches,
        exit_code: args.bench_exit_code,
    };

    write_text(
        &args.report,
        &render::build_report(&run, render::ReportOptions { full: args.full }),
    )?;
    write_json(&args.verdict, &payload::verdict(&run))?;
    println!("{}", policy::verdict(&run).name());
    Ok(())
}

fn gate(args: &GateArgs) -> Result<()> {
    let verdict = read_verdict(&args.verdict)?;
    match verdict
        .get("verdict")
        .and_then(Value::as_str)
        .unwrap_or("error")
    {
        "regressed" | "error" => {
            eprintln!("::error::Benchmark regressions were detected; see the Bench report check.");
            Err(Error::new("benchmark gate failed"))
        }
        _ => Ok(()),
    }
}

fn has_regressions(args: &HasRegressionsArgs) -> Result<()> {
    let verdict = read_verdict(&args.verdict)?;
    let regressed = verdict
        .get("counts")
        .and_then(|counts| counts.get("regressed"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    println!("{}", regressed > 0);
    Ok(())
}

fn read_verdict(path: &Path) -> Result<Value> {
    Ok(json::parse(&fs::read_to_string(path)?)?)
}

fn post_report(args: PostReportArgs) -> Result<()> {
    let options = PublishOptions {
        verdict: args.verdict,
        report: args.report,
        dry_run: args.dry_run,
    };
    publish::prepare(&options)?;
    publish::publish(&options)
}

fn write_text(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    write_text(path, &format!("{}\n", json::to_string_pretty(value)))
}
