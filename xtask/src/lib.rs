//! Workspace automation for the zeta repository.
//!
//! Invoked through the cargo alias defined in `.cargo/config.toml`:
//!
//! ```text
//! cargo xtask bench metadata --output bench-metadata.txt --toolchain stable
//! cargo xtask bench check-config
//! cargo xtask bench report --baseline-found --bench-exit-code 3
//! cargo xtask bench post-report --dry-run
//! ```
//!
//! The binary is a thin wrapper around [`run`]: this library owns argument parsing, task
//! dispatch and the benchmark automation so it can be tested directly.

mod bench;
mod cli;
mod error;
mod json;
mod time;

use std::path::Path;

pub use crate::error::{Error, Result};

/// The repository root, derived from the xtask crate location.
#[must_use]
pub(crate) fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
}

/// Run the task selected on the command line.
///
/// # Errors
///
/// Returns an error when the arguments are invalid or the selected task fails.
pub fn run() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&arguments)? {
        cli::Invocation::Help(topic) => {
            cli::print_usage(topic);
            Ok(())
        }
        cli::Invocation::Command(command) => command.run(),
    }
}
