//! Publish the verdict as a GitHub check run and per-suite commit statuses.
//!
//! Publishing is best-effort: fork pull requests receive a read-only token, so permission
//! errors are warnings instead of failures. The regression gate lives in a separate task so
//! a publishing failure can never mask it. `gh` is used for authenticated requests, keeping
//! this tool free of HTTP/TLS dependencies.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};
use crate::json::{self, Value};
use crate::workspace_root;

pub struct PublishOptions {
    pub verdict: PathBuf,
    pub report: PathBuf,
    pub dry_run: bool,
}

pub fn publish(options: &PublishOptions) -> Result<()> {
    let verdict: Value = json::parse(&fs::read_to_string(&options.verdict)?)?;
    let report = fs::read_to_string(&options.report)?;
    let repository = std::env::var("GITHUB_REPOSITORY").unwrap_or_default();
    let sha = std::env::var("GITHUB_SHA").unwrap_or_default();

    if !options.dry_run && (repository.is_empty() || sha.is_empty()) {
        eprintln!("warning: missing GITHUB_REPOSITORY/GITHUB_SHA; not publishing");
        return Ok(());
    }

    let run_url = run_url();
    let summary: String = report.chars().take(65_000).collect();
    let check_run = Value::object([
        ("name", Value::from("Bench report")),
        ("head_sha", Value::from(sha.as_str())),
        ("status", Value::from("completed")),
        (
            "conclusion",
            Value::from(
                verdict
                    .get("conclusion")
                    .and_then(Value::as_str)
                    .unwrap_or("neutral"),
            ),
        ),
        ("details_url", Value::from(run_url.as_str())),
        (
            "output",
            Value::object([
                (
                    "title",
                    Value::from(
                        verdict
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("Bench report"),
                    ),
                ),
                ("summary", Value::from(summary.as_str())),
                ("annotations", Value::Array(annotations(&verdict))),
            ]),
        ),
    ]);
    post(
        &format!("/repos/{repository}/check-runs"),
        &check_run,
        options.dry_run,
    );

    if let Some(statuses) = verdict.get("statuses").and_then(Value::as_object) {
        for (context, status) in statuses {
            let mut payload = Value::object([
                (
                    "state",
                    Value::from(
                        status
                            .get("state")
                            .and_then(Value::as_str)
                            .unwrap_or("success"),
                    ),
                ),
                ("context", Value::from(context.as_str())),
                (
                    "description",
                    Value::from(
                        status
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                    ),
                ),
            ]);
            if !run_url.is_empty() {
                payload.insert("target_url", run_url.as_str());
            }
            post(
                &format!("/repos/{repository}/statuses/{sha}"),
                &payload,
                options.dry_run,
            );
        }
    }
    Ok(())
}

fn run_url() -> String {
    std::env::var("RUN_URL").unwrap_or_else(|_| {
        let server = std::env::var("GITHUB_SERVER_URL").unwrap_or_default();
        let repository = std::env::var("GITHUB_REPOSITORY").unwrap_or_default();
        let run_id = std::env::var("GITHUB_RUN_ID").unwrap_or_default();
        format!("{server}/{repository}/actions/runs/{run_id}")
    })
}

fn annotations(verdict: &Value) -> Vec<Value> {
    let mut annotations = Vec::new();
    let Some(regressions) = verdict.get("regressions").and_then(Value::as_array) else {
        return annotations;
    };
    for regression in regressions.iter().take(50) {
        let Some(source) = regression.get("source").and_then(Value::as_str) else {
            continue;
        };
        if !workspace_root().join(source).is_file() {
            continue;
        }
        let reasons: Vec<&str> = regression
            .get("reasons")
            .and_then(Value::as_array)
            .map_or_default(|reasons| reasons.iter().filter_map(Value::as_str).collect());
        annotations.push(Value::object([
            ("path", Value::from(source)),
            ("start_line", Value::from(1_u64)),
            ("end_line", Value::from(1_u64)),
            ("annotation_level", Value::from("failure")),
            ("title", Value::from("Benchmark regression")),
            (
                "message",
                Value::from(format!(
                    "{}: {}",
                    regression
                        .get("display")
                        .and_then(Value::as_str)
                        .unwrap_or("benchmark"),
                    reasons.join("; ")
                )),
            ),
        ]));
    }
    annotations
}

fn post(path: &str, payload: &Value, dry_run: bool) {
    if dry_run {
        println!("POST {path}");
        println!("{}", json::to_string_pretty(payload));
        return;
    }

    let mut child = match Command::new("gh")
        .args(["api", path, "--method", "POST", "--input", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("warning: could not run `gh` to publish {path}: {error}");
            return;
        }
    };

    if let Some(stdin) = child.stdin.as_mut()
        && stdin
            .write_all(json::to_string(payload).as_bytes())
            .is_err()
    {
        eprintln!("warning: could not send payload to `gh` for {path}");
    }

    match child.wait_with_output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("warning: publishing {path} failed: {}", stderr.trim());
        }
        Err(error) => eprintln!("warning: publishing {path} failed: {error}"),
    }
}

/// Validate that the report and verdict files exist before publishing.
pub fn prepare(options: &PublishOptions) -> Result<()> {
    exists(&options.verdict)?;
    exists(&options.report)
}

fn exists(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(Error::new(format!(
            "{} does not exist; run `cargo xtask bench report` first",
            path.display()
        )))
    }
}
