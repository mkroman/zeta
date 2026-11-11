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
fn help_lists_every_benchmark_task() {
    let output = xtask(&["--help"]);
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
            "help does not list `{task}`:\n{stdout}"
        );
    }
}

#[test]
fn bench_help_lists_the_task_options() {
    let output = xtask(&["bench", "--help"]);
    assert!(output.status.success());
    let stdout = stdout(&output);
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
