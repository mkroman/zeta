//! The deterministic `manifest.json` that records what one generation run consumed.
//!
//! The manifest names every input file (relative to the data directory), its size and its
//! FNV-1a 64 hash, then every packed section with its file offset and size. It carries no
//! timestamps and no absolute paths, so the same inputs always render the same bytes.

use std::path::{Component, Path};

use serde::Serialize;

use crate::train::TrainOutput;

/// The value written as `format`.
const FORMAT: &str = "zeta-unicode-manifest";
/// The manifest schema version.
const FORMAT_VERSION: u32 = 1;
/// The generator name recorded in the manifest.
const TOOL_NAME: &str = "zeta-unicode-build";

/// The FNV-1a 64 offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// The FNV-1a 64 prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Computes the 64-bit FNV-1a hash of `bytes`.
#[must_use]
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// The serializable `manifest.json` schema.
#[derive(Serialize)]
struct Manifest<'a> {
    /// The schema discriminator.
    format: &'static str,
    /// The schema version.
    format_version: u32,
    /// The generator that produced the image.
    tool: Tool,
    /// The image format discriminator.
    layout: &'static str,
    /// The name section layout.
    name_layout: &'static str,
    /// The Unicode release the inputs advertise.
    unicode_version: (u8, u8, u8),
    /// The independent Emoji release of `emoji-test.txt`.
    emoji_version: (u8, u8),
    /// The chosen training profile.
    profile: ProfileEntry<'a>,
    /// The packed image sections.
    sections: &'a [crate::train::SectionRecord],
    /// The complete image size.
    total_bytes: usize,
    /// The hashed UCD inputs.
    inputs: &'a [InputRecord],
    /// Every measured name-block candidate.
    candidates: &'a [crate::train::Candidate],
    /// The checks the training run performed.
    verification: Verification<'a>,
}

/// The generator name and version.
#[derive(Serialize)]
struct Tool {
    /// The generator crate name.
    name: &'static str,
    /// The generator crate version.
    version: &'static str,
}

/// The chosen profile and its parameters.
#[derive(Serialize)]
struct ProfileEntry<'a> {
    /// The priority rule that selected the candidate.
    name: &'static str,
    /// The resolved profile parameters.
    parameters: ProfileParameters<'a>,
}

/// The resolved profile parameters.
#[derive(Serialize)]
struct ProfileParameters<'a> {
    /// The name-block shift the image was built with.
    name_block_shift: u8,
    /// The image budget the run was checked against.
    max_table_bytes: usize,
    /// The tolerated growth over a baseline.
    max_delta_bytes: usize,
    /// Every candidate shift the run measured.
    candidates: &'a [u8],
}

/// The verification status and checks.
#[derive(Serialize)]
struct Verification<'a> {
    /// The status written for a successful run.
    status: &'static str,
    /// The checks that ran.
    checks: &'a [&'static str],
}

/// Writes `manifest.json` for one Nomina training run.
///
/// The Nomina image has no name-derivation rules file, so the manifest records only the UCD
/// inputs: each one's path relative to `data_dir`, its size and its FNV-1a 64 hash.
/// It also records the four image sections, the chosen profile, every measured candidate and
/// the verification status. There are no timestamps and no absolute paths, so two runs over
/// the same inputs render identical bytes.
///
/// # Errors
///
/// Returns [`Error`](crate::Error) when an input cannot be read, when an input path is not
/// inside `data_dir`, when the manifest cannot be serialized, or when `manifest.json` cannot be
/// written.
pub fn write_manifest(
    data_dir: &Path,
    out_dir: &Path,
    inputs: &crate::ucd::Inputs,
    output: &TrainOutput,
) -> Result<(), crate::Error> {
    let mut records = input_records(data_dir, inputs)?;
    records.sort();
    let text = render_manifest(inputs, output, &records)?;
    crate::write(out_dir.join("manifest.json"), text)
}

/// Renders the Nomina manifest for one training run.
///
/// # Errors
///
/// Returns [`Error::Json`](crate::Error::Json) when the schema cannot be serialized.
fn render_manifest(
    inputs: &crate::ucd::Inputs,
    output: &TrainOutput,
    records: &[InputRecord],
) -> Result<String, crate::Error> {
    let name_layout = match output.names.layout {
        crate::nomina::names::NameLayout::BlockLocal => "block-local",
        crate::nomina::names::NameLayout::Global => "global-checkpointed",
    };
    let manifest = Manifest {
        format: FORMAT,
        format_version: FORMAT_VERSION,
        tool: Tool {
            name: TOOL_NAME,
            version: env!("CARGO_PKG_VERSION"),
        },
        layout: "nomina",
        name_layout,
        unicode_version: inputs.version,
        emoji_version: inputs.emoji_version,
        profile: ProfileEntry {
            name: output.profile.priority.as_str(),
            parameters: ProfileParameters {
                name_block_shift: output.profile.name_block_shift,
                max_table_bytes: output.profile.max_table_bytes,
                max_delta_bytes: output.profile.max_delta_bytes,
                candidates: &output.profile.candidates,
            },
        },
        sections: &output.sections,
        total_bytes: output.total_bytes,
        inputs: records,
        candidates: &output.candidates,
        verification: Verification {
            status: "verified",
            checks: output.verification,
        },
    };
    let mut text = serde_json::to_string_pretty(&manifest)?;
    text.push('\n');
    Ok(text)
}

/// Collects the input records for a Nomina run, hashing the bytes on disk.
///
/// The on-disk bytes are the actual inputs a rebuild needs, so they are hashed rather than any
/// in-memory text derived from them.
fn input_records(
    data_dir: &Path,
    inputs: &crate::ucd::Inputs,
) -> Result<Vec<InputRecord>, crate::Error> {
    let mut paths = vec![
        inputs.paths.names.as_path(),
        inputs.paths.gc.as_path(),
        inputs.paths.aliases.as_path(),
        inputs.paths.emoji.as_path(),
    ];
    if let Some(path) = &inputs.paths.sequences {
        paths.push(path.as_path());
    }
    let mut records = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = std::fs::read(path).map_err(|source| crate::Error::Read {
            path: path.to_path_buf(),
            source,
        })?;
        records.push(record(path, &bytes, data_dir)?);
    }
    Ok(records)
}

/// One input file recorded in the manifest.
#[derive(PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct InputRecord {
    /// The file name as written by Unicode.
    name: String,
    /// The path relative to the data directory, with `/` separators.
    path: String,
    /// The input's byte length.
    size: usize,
    /// The input's FNV-1a 64 hash, as sixteen lower-case hex digits.
    #[serde(rename = "fnv1a64")]
    hash: String,
}

/// Builds one manifest record from a resolved path and its contents.
///
/// # Errors
///
/// Returns [`Error::OutsideDataDir`] when `path` is not inside `data_dir`: the manifest must
/// never fall back to a wrong shorter path.
fn record(path: &Path, bytes: &[u8], data_dir: &Path) -> Result<InputRecord, crate::Error> {
    Ok(InputRecord {
        name: path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        path: relative_path(path, data_dir)?,
        size: bytes.len(),
        hash: format!("{:016x}", fnv1a64(bytes)),
    })
}

/// Renders `path` relative to `data_dir`, with `/` separators and no absolute components.
///
/// # Errors
///
/// Returns [`Error::OutsideDataDir`] when `path` is not strictly inside `data_dir`. The
/// manifest records where a rebuild must place every input, so a silently shortened path
/// would lie about that.
fn relative_path(path: &Path, data_dir: &Path) -> Result<String, crate::Error> {
    let outside = || crate::Error::OutsideDataDir {
        path: path.to_path_buf(),
        data_dir: data_dir.to_path_buf(),
    };
    let relative = path.strip_prefix(data_dir).map_err(|_| outside())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            // A root, parent or current-directory component means the path was not written
            // relative to the data directory; a silently shortened path would lie about where
            // a rebuild must put the input.
            _ => return Err(outside()),
        }
    }
    if parts.is_empty() {
        return Err(outside());
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_under_the_data_dir_renders_with_slash_separators() {
        let path = Path::new("/data/ucd/extracted/DerivedName.txt");
        let rendered = relative_path(path, Path::new("/data")).expect("a path under the data dir");
        assert_eq!(rendered, "ucd/extracted/DerivedName.txt");
    }

    #[test]
    fn a_path_outside_the_data_dir_is_an_error() {
        for (path, data_dir) in [
            ("/other/DerivedName.txt", "/data"),
            ("/data", "/data"),
            ("/database/DerivedName.txt", "/data"),
            ("/data/../DerivedName.txt", "/data"),
        ] {
            let error = relative_path(Path::new(path), Path::new(data_dir))
                .expect_err("the path is not strictly inside the data dir");
            assert!(
                matches!(error, crate::Error::OutsideDataDir { .. }),
                "{path} under {data_dir}: {error}"
            );
        }
    }

    #[test]
    fn the_input_hash_has_the_published_fnv_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn an_empty_data_dir_takes_the_whole_relative_path() {
        let rendered = relative_path(Path::new("data/DerivedName.txt"), Path::new(""))
            .expect("an empty prefix matches every relative path");
        assert_eq!(rendered, "data/DerivedName.txt");
    }
}
