//! Workspace automation for the zeta repository.
//!
//! Invoked through the cargo alias defined in `.cargo/config.toml`:
//!
//! ```text
//! cargo xtask bench metadata --output bench-metadata.txt --toolchain stable
//! cargo xtask bench check-config
//! cargo xtask bench report --baseline-found --bench-exit-code 3
//! cargo xtask bench publish --dry-run
//! ```

mod args;
mod bench;
mod check;
mod config;
mod error;
mod format;
mod metadata;
mod model;
mod policy;
mod publish;
mod render;

use std::path::Path;

use crate::error::{Error, Result};

/// The repository root, derived from the xtask crate location.
#[must_use]
pub fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("bench") => bench::run(&arguments[1..]),
        Some("-h" | "--help") => {
            print_usage();
            Ok(())
        }
        _ => {
            print_usage();
            Err(Error::new(
                "expected a task, for example `cargo xtask bench check-config`",
            ))
        }
    }
}

fn print_usage() {
    println!(
        "Usage: cargo xtask bench <command>

Commands:
  metadata      Record the benchmark environment metadata
  check-config  Validate the report configuration against the measured benchmarks
  report        Render the benchmark report and verdict from Gungraun summaries
  publish       Publish the verdict as a check run and commit statuses"
    );
}
