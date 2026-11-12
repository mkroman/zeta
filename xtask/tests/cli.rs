//! End-to-end smoke tests for the command-line surface.
//!
//! These run the real binary so the cargo alias, argument parsing and exit codes are covered
//! in the same way the CI workflows invoke them.

use std::process::{Command, Output};

fn xtask(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(arguments)
        .output()
        .expect("run the xtask binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn top_level_help_lists_tasks() {
    let output = xtask(&["--help"]);
    assert!(output.status.success());
    let stdout = stdout(&output);
    for task in ["bench", "fetch-ucd"] {
        assert!(
            stdout.contains(task),
            "help does not list `{task}`:\n{stdout}"
        );
    }
    for option in ["--version", "--dest", "--emoji-version"] {
        assert!(
            stdout.contains(option),
            "help does not document `{option}`:\n{stdout}"
        );
    }
}

#[test]
fn bench_help_lists_the_task_options() {
    let output = xtask(&["bench", "--help"]);
    assert!(output.status.success());
    let stdout = stdout(&output);
    for task in [
        "metadata",
        "check-config",
        "report",
        "gate",
        "has-regressions",
        "post-report",
    ] {
        assert!(
            stdout.contains(task),
            "bench help does not list `{task}`:\n{stdout}"
        );
    }
    for option in [
        "--output",
        "--summaries",
        "--baseline-found",
        "--bench-exit-code",
        "--verdict",
        "--dry-run",
    ] {
        assert!(
            stdout.contains(option),
            "bench help does not list `{option}`:\n{stdout}"
        );
    }
}

#[test]
fn missing_task_prints_usage_and_fails() {
    let output = xtask(&[]);
    assert!(!output.status.success());
    assert!(stdout(&output).contains("Usage: cargo xtask"));
    assert!(stderr(&output).contains("expected a task"));
}

#[test]
fn missing_benchmark_task_prints_usage_and_fails() {
    let output = xtask(&["bench"]);
    assert!(!output.status.success());
    assert!(stdout(&output).contains("Usage: cargo xtask bench"));
    assert!(stderr(&output).contains("expected a benchmark task"));
}

#[test]
fn unknown_benchmark_task_fails() {
    let output = xtask(&["bench", "nonsense"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown benchmark task `nonsense`"));
}

#[test]
fn fetch_ucd_requires_version_and_dest() {
    let output = xtask(&["fetch-ucd"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("missing required option `--version`"));
}

#[test]
fn fetch_ucd_rejects_unknown_options_before_downloading() {
    let output = xtask(&[
        "fetch-ucd",
        "--version",
        "18.0.0",
        "--dest",
        "target/ucd-test",
        "--unknown",
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown option `--unknown`"));
}
