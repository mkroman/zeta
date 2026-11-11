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

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::workspace_root;

pub struct PublishOptions {
    pub verdict: PathBuf,
    pub report: PathBuf,
    pub dry_run: bool,
}

pub fn publish(options: &PublishOptions) -> Result<()> {
    let verdict: Value = serde_json::from_str(&fs::read_to_string(&options.verdict)?)?;
    let report = fs::read_to_string(&options.report)?;
    let repository = std::env::var("GITHUB_REPOSITORY").unwrap_or_default();
    let sha = std::env::var("GITHUB_SHA").unwrap_or_default();

    if !options.dry_run && (repository.is_empty() || sha.is_empty()) {
        eprintln!("warning: missing GITHUB_REPOSITORY/GITHUB_SHA; not publishing");
        return Ok(());
    }

    let run_url = run_url();
    let summary: String = report.chars().take(65_000).collect();
    let check_run = json!({
        "name": "Bench report",
        "head_sha": sha,
        "status": "completed",
        "conclusion": verdict["conclusion"].as_str().unwrap_or("neutral"),
        "details_url": run_url,
        "output": {
            "title": verdict["title"].as_str().unwrap_or("Bench report"),
            "summary": summary,
            "annotations": annotations(&verdict),
        }
    });
    post(
        &format!("/repos/{repository}/check-runs"),
        &check_run,
        options.dry_run,
    );

    if let Some(statuses) = verdict["statuses"].as_object() {
        for (context, status) in statuses {
            let mut payload = json!({
                "state": status["state"].as_str().unwrap_or("success"),
                "context": context,
                "description": status["description"].as_str().unwrap_or(""),
            });
            if !run_url.is_empty() {
                payload["target_url"] = Value::String(run_url.clone());
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
    let Some(regressions) = verdict["regressions"].as_array() else {
        return annotations;
    };
    for regression in regressions.iter().take(50) {
        let Some(source) = regression["source"].as_str() else {
            continue;
        };
        if !workspace_root().join(source).is_file() {
            continue;
        }
        let reasons: Vec<&str> = regression["reasons"]
            .as_array()
            .map(|reasons| reasons.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        annotations.push(json!({
            "path": source,
            "start_line": 1,
            "end_line": 1,
            "annotation_level": "failure",
            "title": "Benchmark regression",
            "message": format!(
                "{}: {}",
                regression["display"].as_str().unwrap_or("benchmark"),
                reasons.join("; ")
            ),
        }));
    }
    annotations
}

fn post(path: &str, payload: &Value, dry_run: bool) {
    if dry_run {
        println!("POST {path}");
        println!(
            "{}",
            serde_json::to_string_pretty(payload).unwrap_or_default()
        );
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

    if let Some(stdin) = child.stdin.as_mut() {
        let body = serde_json::to_vec(payload).unwrap_or_default();
        if stdin.write_all(&body).is_err() {
            eprintln!("warning: could not send payload to `gh` for {path}");
        }
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
