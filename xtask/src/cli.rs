//! Typed command-line parsing for the xtask binary.
//!
//! The surface is intentionally small and dependency-free: the only shapes are `--flag`
//! switches and `--option value` pairs. Each task parses into its own argument struct, so
//! the task handlers never see raw strings or untrusted option names.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::workspace_root;

/// A parsed command line.
pub enum Invocation {
    /// Print usage for the topic and exit successfully.
    Help(HelpTopic),
    /// Run a task.
    Command(Command),
}

/// The usage text a help request refers to.
#[derive(Clone, Copy)]
pub enum HelpTopic {
    TopLevel,
    Bench,
}

/// A top-level xtask task.
pub enum Command {
    Bench(BenchCommand),
}

impl Command {
    /// Run this command.
    pub fn run(self) -> Result<()> {
        match self {
            Self::Bench(command) => crate::bench::run(command),
        }
    }
}

/// A `cargo xtask bench` task and its validated arguments.
pub enum BenchCommand {
    Metadata(MetadataArgs),
    CheckConfig(CheckConfigArgs),
    Report(ReportArgs),
    Gate(GateArgs),
    HasRegressions(HasRegressionsArgs),
    PostReport(PostReportArgs),
}

/// Arguments for `bench metadata`.
pub struct MetadataArgs {
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
pub struct CheckConfigArgs {
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
pub struct ReportArgs {
    pub summaries: PathBuf,
    pub baseline_metadata: PathBuf,
    pub current_metadata: PathBuf,
    pub baseline_found: bool,
    pub bench_exit_code: i32,
    pub report: PathBuf,
    pub verdict: PathBuf,
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
            &["baseline-found"],
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
        })
    }
}

/// Arguments for `bench gate`.
pub struct GateArgs {
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
pub struct HasRegressionsArgs {
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
pub struct PostReportArgs {
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

/// Parse the arguments passed after the `cargo xtask` alias.
pub fn parse(arguments: &[String]) -> Result<Invocation> {
    let Some((task, rest)) = arguments.split_first() else {
        print_usage(HelpTopic::TopLevel);
        return Err(Error::new(
            "expected a task, for example `cargo xtask bench check-config`",
        ));
    };
    match task.as_str() {
        "bench" => parse_bench(rest),
        "-h" | "--help" => Ok(Invocation::Help(HelpTopic::TopLevel)),
        _ => {
            print_usage(HelpTopic::TopLevel);
            Err(Error::new(
                "expected a task, for example `cargo xtask bench check-config`",
            ))
        }
    }
}

fn parse_bench(arguments: &[String]) -> Result<Invocation> {
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

/// Print the usage text for `topic`.
pub fn print_usage(topic: HelpTopic) {
    match topic {
        HelpTopic::TopLevel => println!(
            "Usage: cargo xtask bench <command>

Commands:
  metadata         Record the benchmark environment metadata
  check-config     Validate the report configuration against the measured benchmarks
  report           Render the benchmark report and verdict from Gungraun summaries
  gate             Fail unless the verdict is passing
  has-regressions  Print true when the verdict contains regressions
  post-report      Post the verdict as a check run and commit statuses"
        ),
        HelpTopic::Bench => println!(
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
        ),
    }
}

/// Parsed `--flag` and `--option value` arguments for one task.
struct Options {
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl Options {
    fn parse(arguments: &[String], value_names: &[&str], flag_names: &[&str]) -> Result<Self> {
        let mut values = BTreeMap::new();
        let mut flags = BTreeSet::new();
        let mut iter = arguments.iter();

        while let Some(argument) = iter.next() {
            let Some(name) = argument.strip_prefix("--") else {
                return Err(Error::new(format!("unexpected argument `{argument}`")));
            };

            if flag_names.contains(&name) {
                flags.insert(name.to_owned());
            } else if value_names.contains(&name) {
                let value = iter
                    .next()
                    .ok_or_else(|| Error::new(format!("option `--{name}` requires a value")))?;
                if values.insert(name.to_owned(), value.clone()).is_some() {
                    return Err(Error::new(format!("option `--{name}` was given twice")));
                }
            } else {
                return Err(Error::new(format!("unknown option `--{name}`")));
            }
        }

        Ok(Self { values, flags })
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    fn path_or(&self, name: &str, default: &str) -> PathBuf {
        resolve_from_root(self.value(name).unwrap_or(default))
    }
}

/// Resolve a possibly relative path against the workspace root instead of the caller's cwd.
fn resolve_from_root(path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        workspace_root().join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(arguments: &[&str]) -> Vec<String> {
        arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect()
    }

    fn bench(arguments: &[&str]) -> Result<BenchCommand> {
        match parse(&self::arguments(arguments))? {
            Invocation::Command(Command::Bench(command)) => Ok(command),
            Invocation::Help(_) => panic!("expected a command"),
        }
    }

    #[test]
    fn applies_report_defaults() {
        let Ok(BenchCommand::Report(args)) = bench(&["bench", "report"]) else {
            panic!("expected report arguments");
        };
        assert_eq!(args.summaries, workspace_root().join("target/gungraun"));
        assert_eq!(
            args.baseline_metadata,
            workspace_root().join("target/gungraun/bench-metadata.txt")
        );
        assert_eq!(
            args.current_metadata,
            workspace_root().join("bench-metadata.txt")
        );
        assert_eq!(args.report, workspace_root().join("bench-report.md"));
        assert_eq!(args.verdict, workspace_root().join("verdict.json"));
        assert!(!args.baseline_found);
        assert_eq!(args.bench_exit_code, 0);
    }

    #[test]
    fn parses_explicit_report_arguments() {
        let Ok(BenchCommand::Report(args)) = bench(&[
            "bench",
            "report",
            "--summaries",
            "custom/summaries",
            "--baseline-found",
            "--bench-exit-code",
            "3",
            "--verdict",
            "/tmp/verdict.json",
        ]) else {
            panic!("expected report arguments");
        };
        assert_eq!(args.summaries, workspace_root().join("custom/summaries"));
        assert!(args.baseline_found);
        assert_eq!(args.bench_exit_code, 3);
        assert_eq!(args.verdict, PathBuf::from("/tmp/verdict.json"));
    }

    #[test]
    fn rejects_invalid_arguments() {
        for arguments in [
            &["bench", "report", "--bench-exit-code", "three"][..],
            &["bench", "report", "--summaries"][..],
            &["bench", "report", "--summaries", "a", "--summaries", "b"][..],
            &["bench", "report", "--unknown"][..],
            &["bench", "report", "positional"][..],
        ] {
            assert!(
                bench(arguments).is_err(),
                "{arguments:?} should be rejected"
            );
        }
    }

    #[test]
    fn recognizes_help_and_unknown_tasks() {
        assert!(matches!(
            parse(&arguments(&["--help"])),
            Ok(Invocation::Help(HelpTopic::TopLevel))
        ));
        assert!(matches!(
            parse(&arguments(&["bench", "--help"])),
            Ok(Invocation::Help(HelpTopic::Bench))
        ));
        assert!(parse(&arguments(&["bench", "unknown"])).is_err());
    }
}
