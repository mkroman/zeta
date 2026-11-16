//! Arguments for the `bench` subcommand and its tasks.
//!
//! Every task parses into its own struct here; the parent module maps task names to these
//! parsers and owns the usage text.

use std::path::PathBuf;

use super::{Command, HelpTopic, Invocation, Options, print_usage};
use crate::error::{Error, Result};

/// A `cargo xtask bench` task and its validated arguments.
pub(crate) enum BenchCommand {
    Metadata(MetadataArgs),
    CheckConfig(CheckConfigArgs),
    Report(ReportArgs),
    Gate(GateArgs),
    HasRegressions(HasRegressionsArgs),
    PostReport(PostReportArgs),
}

/// Arguments for `bench metadata`.
pub(crate) struct MetadataArgs {
    pub output: PathBuf,
    pub toolchain: String,
}

impl MetadataArgs {
    fn parse(arguments: &[String]) -> Result<Self> {
        let options = Options::parse(arguments, &["output", "toolchain"], &[])?;
        Ok(Self {
            output: options.path_or("output", "bench-metadata.txt"),
            toolchain: options.value("toolchain").unwrap_or("stable").to_owned(),
        })
    }
}

/// Arguments for `bench check-config`.
pub(crate) struct CheckConfigArgs {
    pub summaries: PathBuf,
}

impl CheckConfigArgs {
    fn parse(arguments: &[String]) -> Result<Self> {
        let options = Options::parse(arguments, &["summaries"], &[])?;
        Ok(Self {
            summaries: options.path_or("summaries", "target/gungraun"),
        })
    }
}

/// Arguments for `bench report`.
pub(crate) struct ReportArgs {
    pub summaries: PathBuf,
    pub baseline_metadata: PathBuf,
    pub current_metadata: PathBuf,
    pub baseline_found: bool,
    pub bench_exit_code: i32,
    pub report: PathBuf,
    pub verdict: PathBuf,
    pub full: bool,
}

impl ReportArgs {
    fn parse(arguments: &[String]) -> Result<Self> {
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
            &["baseline-found", "full"],
        )?;
        let bench_exit_code = options.value("bench-exit-code").map_or(Ok(0), |value| {
            value
                .parse::<i32>()
                .map_err(|_| Error::new(format!("invalid --bench-exit-code `{value}`")))
        })?;
        Ok(Self {
            summaries: options.path_or("summaries", "target/gungraun"),
            baseline_metadata: options
                .path_or("baseline-metadata", "target/gungraun/bench-metadata.txt"),
            current_metadata: options.path_or("current-metadata", "bench-metadata.txt"),
            baseline_found: options.flag("baseline-found"),
            bench_exit_code,
            report: options.path_or("report", "bench-report.md"),
            verdict: options.path_or("verdict", "verdict.json"),
            full: options.flag("full"),
        })
    }
}

/// Arguments for `bench gate`.
pub(crate) struct GateArgs {
    pub verdict: PathBuf,
}

impl GateArgs {
    fn parse(arguments: &[String]) -> Result<Self> {
        let options = Options::parse(arguments, &["verdict"], &[])?;
        Ok(Self {
            verdict: options.path_or("verdict", "verdict.json"),
        })
    }
}

/// Arguments for `bench has-regressions`.
pub(crate) struct HasRegressionsArgs {
    pub verdict: PathBuf,
}

impl HasRegressionsArgs {
    fn parse(arguments: &[String]) -> Result<Self> {
        let options = Options::parse(arguments, &["verdict"], &[])?;
        Ok(Self {
            verdict: options.path_or("verdict", "verdict.json"),
        })
    }
}

/// Arguments for `bench post-report`.
pub(crate) struct PostReportArgs {
    pub verdict: PathBuf,
    pub report: PathBuf,
    pub dry_run: bool,
}

impl PostReportArgs {
    fn parse(arguments: &[String]) -> Result<Self> {
        let options = Options::parse(arguments, &["verdict", "report"], &["dry-run"])?;
        Ok(Self {
            verdict: options.path_or("verdict", "verdict.json"),
            report: options.path_or("report", "bench-report.md"),
            dry_run: options.flag("dry-run"),
        })
    }
}

/// Parse the arguments that follow `bench`.
pub(super) fn parse(arguments: &[String]) -> Result<Invocation> {
    let Some((task, rest)) = arguments.split_first() else {
        print_usage(HelpTopic::Bench);
        return Err(Error::new("expected a benchmark task"));
    };
    let command = match task.as_str() {
        "metadata" => BenchCommand::Metadata(MetadataArgs::parse(rest)?),
        "check-config" => BenchCommand::CheckConfig(CheckConfigArgs::parse(rest)?),
        "report" => BenchCommand::Report(ReportArgs::parse(rest)?),
        "gate" => BenchCommand::Gate(GateArgs::parse(rest)?),
        "has-regressions" => BenchCommand::HasRegressions(HasRegressionsArgs::parse(rest)?),
        "post-report" => BenchCommand::PostReport(PostReportArgs::parse(rest)?),
        "-h" | "--help" => return Ok(Invocation::Help(HelpTopic::Bench)),
        other => return Err(Error::new(format!("unknown benchmark task `{other}`"))),
    };
    Ok(Invocation::Command(Command::Bench(command)))
}
