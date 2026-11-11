//! Benchmark environment metadata: recording and comparison.
//!
//! Instruction counts are only comparable when the compiler, Valgrind and gungraun versions
//! match the recorded baseline, so the metadata is stored next to the counts and checked
//! before gating.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::config;
use crate::error::{Error, Result};
use crate::workspace_root;

/// Toolchain metadata that must match the baseline for exact-count gating.
const TOOLCHAIN_KEYS: &[&str] = &[
    "toolchain",
    "rustc-release",
    "rustc-commit",
    "valgrind",
    "gungraun-runner",
];

/// A key-value view over the recorded environment.
#[derive(Debug, Default)]
pub struct Metadata {
    values: BTreeMap<String, String>,
}

/// One metadata key that differs between the baseline and the current run.
pub struct Mismatch {
    pub key: String,
    pub baseline: String,
    pub current: String,
}

impl Metadata {
    /// Read a metadata file; a missing file yields empty metadata (no baseline recorded).
    pub fn read(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        Ok(Self::parse(&fs::read_to_string(path)?))
    }

    fn parse(text: &str) -> Self {
        let mut values = BTreeMap::new();
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                values.insert(key.trim().to_owned(), value.trim().to_owned());
            }
        }
        Self { values }
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = String::new();
        for (key, value) in &self.values {
            text.push_str(key);
            text.push('=');
            text.push_str(value);
            text.push('\n');
        }
        fs::write(path, text)?;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map_or("", String::as_str)
    }

    /// Age of the baseline in whole days, if a timestamp was recorded.
    pub fn age_days(&self) -> Option<i64> {
        self.get("unix-time")
            .parse::<i64>()
            .ok()
            .map(|timestamp| (config::unix_now() - timestamp).max(0) / 86_400)
    }

    /// Differences in the keys that make instruction counts comparable. Without metadata on
    /// both sides there is nothing to compare.
    pub fn mismatches(&self, current: &Self) -> Vec<Mismatch> {
        if self.is_empty() || current.is_empty() {
            return Vec::new();
        }
        TOOLCHAIN_KEYS
            .iter()
            .filter_map(|&key| {
                let baseline = self.get(key).to_owned();
                let current = current.get(key).to_owned();
                (baseline != current).then_some(Mismatch {
                    key: key.to_owned(),
                    baseline,
                    current,
                })
            })
            .collect()
    }
}

/// Record the environment that produced the counts into `output`.
pub fn record(output: &Path, toolchain: &str) -> Result<()> {
    let root = workspace_root();
    let mut values = BTreeMap::new();

    values.insert(
        "commit".to_owned(),
        run_command("git", &["-C", path_str(root), "rev-parse", "HEAD"])?,
    );
    values.insert("unix-time".to_owned(), config::unix_now().to_string());
    values.insert("toolchain".to_owned(), toolchain.to_owned());

    let rustc = run_command("rustc", &["-vV"])?;
    for line in rustc.lines() {
        if let Some(release) = line.strip_prefix("release: ") {
            values.insert("rustc-release".to_owned(), release.to_owned());
        } else if let Some(commit) = line.strip_prefix("commit-hash: ") {
            values.insert("rustc-commit".to_owned(), commit.to_owned());
        }
    }

    values.insert(
        "valgrind".to_owned(),
        run_command("valgrind", &["--version"])?,
    );
    values.insert(
        "gungraun-runner".to_owned(),
        run_command("gungraun-runner", &["--version"])?,
    );
    values.insert(
        "cargo-lock-hash".to_owned(),
        run_command("git", &["-C", path_str(root), "hash-object", "Cargo.lock"])?,
    );

    if let Ok(value) = std::env::var("GITHUB_RUN_ID") {
        values.insert("run-id".to_owned(), value);
    }
    let run_url = run_url();
    if let Some(url) = run_url {
        values.insert("run-url".to_owned(), url);
    }

    Metadata { values }.write(output)
}

fn run_url() -> Option<String> {
    let server = std::env::var("GITHUB_SERVER_URL").ok()?;
    let repository = std::env::var("GITHUB_REPOSITORY").ok()?;
    let run_id = std::env::var("GITHUB_RUN_ID").ok()?;
    Some(format!("{server}/{repository}/actions/runs/{run_id}"))
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap_or(".")
}

fn run_command(program: &str, arguments: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| Error::new(format!("failed to run `{program}`: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(format!("`{program}` failed: {}", stderr.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_only_shared_toolchain_keys() {
        let baseline = Metadata {
            values: BTreeMap::from([
                ("toolchain".to_owned(), "stable".to_owned()),
                ("rustc-commit".to_owned(), "a".to_owned()),
                ("commit".to_owned(), "baseline".to_owned()),
            ]),
        };
        let current = Metadata {
            values: BTreeMap::from([
                ("toolchain".to_owned(), "stable".to_owned()),
                ("rustc-commit".to_owned(), "b".to_owned()),
                ("commit".to_owned(), "current".to_owned()),
            ]),
        };
        let mismatches = baseline.mismatches(&current);
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].key, "rustc-commit");
    }

    #[test]
    fn missing_metadata_has_no_mismatches() {
        let current = Metadata {
            values: BTreeMap::from([("toolchain".to_owned(), "stable".to_owned())]),
        };
        assert!(Metadata::default().mismatches(&current).is_empty());
    }

    #[test]
    fn parses_metadata_files() {
        let metadata = Metadata::parse("toolchain=stable\ncommit=abc\n");
        assert_eq!(metadata.get("toolchain"), "stable");
        assert_eq!(metadata.get("commit"), "abc");
    }
}
