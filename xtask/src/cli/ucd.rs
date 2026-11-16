//! Arguments for the `fetch-ucd` task.

use std::path::PathBuf;

use super::Options;
use crate::error::{Error, Result};

/// Arguments for `fetch-ucd`.
pub(crate) struct FetchUcdArgs {
    pub version: String,
    pub dest: PathBuf,
    pub emoji_version: Option<String>,
}

/// Parse the arguments that follow `fetch-ucd`.
pub(super) fn parse(arguments: &[String]) -> Result<FetchUcdArgs> {
    let options = Options::parse(arguments, &["version", "dest", "emoji-version"], &[])?;
    Ok(FetchUcdArgs {
        version: options.required_value("version")?.to_owned(),
        dest: options
            .path("dest")
            .ok_or_else(|| Error::new("missing required option `--dest`"))?,
        emoji_version: options.value("emoji-version").map(str::to_owned),
    })
}
