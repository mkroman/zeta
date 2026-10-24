//! The deterministic `manifest.json` that records what one generation run consumed.
//!
//! The manifest names every input file (relative to the data directory), its size and its
//! FNV-1a 64 hash, then every packed section with its file offset and size. It carries no
//! timestamps and no absolute paths, so the same inputs always render the same bytes.

use std::fmt::Write as _;
use std::path::{Component, Path};

use crate::train::{TrainOutput, Verification};

/// The value written as `format`.
const FORMAT: &str = "zeta-unicode-manifest";
/// The manifest schema version.
const FORMAT_VERSION: u32 = 1;

/// The FNV-1a 64 offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// The FNV-1a 64 prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Computes the 64-bit FNV-1a hash of `bytes`.
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// Appends the tool name and version shared by the manifest.
fn push_tool(out: &mut String) {
    out.push_str("  \"tool\": {\n    \"name\": ");
    push_json_string(out, "zeta-unicode-build");
    out.push_str(",\n    \"version\": ");
    push_json_string(out, env!("CARGO_PKG_VERSION"));
    out.push_str("\n  },\n");
}

/// Appends the Unicode and Emoji versions to the manifest.
fn push_versions(out: &mut String, inputs: &crate::ucd::Inputs) {
    let _ = writeln!(
        out,
        "  \"unicode_version\": [{}, {}, {}],",
        inputs.version.0, inputs.version.1, inputs.version.2
    );
    let _ = writeln!(
        out,
        "  \"emoji_version\": [{}, {}],",
        inputs.emoji_version.0, inputs.emoji_version.1
    );
}

/// Writes `manifest.json` for one Genesis Lean training run.
///
/// The Lean image has no name-derivation rules file, so the manifest records only the UCD
/// inputs: each one's path relative to `data_dir`, its in-memory size and its FNV-1a 64 hash.
/// It also records the four image sections, the chosen profile, every measured candidate and
/// the verification status. There are no timestamps and no absolute paths, so two runs over
/// the same inputs render identical bytes.
///
/// # Errors
///
/// Returns [`Error`](crate::Error) when `manifest.json` cannot be written.
pub fn write_lean(
    data_dir: &Path,
    out_dir: &Path,
    inputs: &crate::ucd::Inputs,
    output: &TrainOutput,
) -> Result<(), crate::Error> {
    let mut records = lean_records(data_dir, inputs)?;
    records.sort();
    let text = render_lean(inputs, output, &records);
    crate::write(out_dir.join("manifest.json"), text)
}

/// Renders the Lean manifest for one training run.
#[allow(
    clippy::too_many_lines,
    reason = "one linear JSON rendering pass, like the legacy manifest"
)]
fn render_lean(
    inputs: &crate::ucd::Inputs,
    output: &TrainOutput,
    records: &[InputRecord],
) -> String {
    let mut out = String::with_capacity(4_096);
    out.push_str("{\n  \"format\": ");
    push_json_string(&mut out, FORMAT);
    let _ = writeln!(out, ",\n  \"format_version\": {FORMAT_VERSION},");
    push_tool(&mut out);
    out.push_str("  \"layout\": \"genesis-lean\",\n");
    out.push_str("  \"name_layout\": ");
    push_json_string(
        &mut out,
        match output.names.layout {
            crate::genesis::names::NameLayout::BlockLocal => "block-local",
            crate::genesis::names::NameLayout::Global => "global-checkpointed",
        },
    );
    out.push_str(",\n");
    push_versions(&mut out, inputs);

    out.push_str("  \"profile\": {\n    \"name\": ");
    push_json_string(&mut out, output.profile.priority.as_str());
    let _ = write!(
        out,
        ",\n    \"parameters\": {{\n      \"name_block_shift\": {}, ",
        output.profile.name_block_shift
    );
    let _ = write!(
        out,
        "\"max_table_bytes\": {}, \"max_delta_bytes\": {}, ",
        output.profile.max_table_bytes, output.profile.max_delta_bytes
    );
    out.push_str("\"candidates\": [");
    for (index, shift) in output.profile.candidates.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "{shift}");
    }
    out.push_str("]\n    }\n  },\n");

    out.push_str("  \"sections\": [");
    for (index, section) in output.sections.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(out, "\n    {{\"id\": {}, \"name\": ", section.id);
        push_json_string(&mut out, section.name);
        let _ = write!(
            out,
            ", \"offset\": {}, \"size\": {}}}",
            section.offset, section.size
        );
    }
    if !output.sections.is_empty() {
        out.push_str("\n  ");
    }
    out.push_str("],\n");

    let _ = writeln!(out, "  \"total_bytes\": {},", output.total_bytes);

    out.push_str("  \"inputs\": [");
    for (index, record) in records.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("\n    {\"name\": ");
        push_json_string(&mut out, &record.name);
        out.push_str(", \"path\": ");
        push_json_string(&mut out, &record.path);
        let _ = write!(
            out,
            ", \"size\": {}, \"fnv1a64\": \"{:016x}\"}}",
            record.size, record.hash
        );
    }
    if !records.is_empty() {
        out.push_str("\n  ");
    }
    out.push_str("],\n");

    out.push_str("  \"candidates\": [");
    for (index, candidate) in output.candidates.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "\n    {{\"name_block_shift\": {}, \"name_bytes\": {}, \"total_bytes\": {}, \
             \"chosen\": {}}}",
            candidate.name_block_shift,
            candidate.name_bytes,
            candidate.total_bytes,
            candidate.chosen
        );
    }
    if !output.candidates.is_empty() {
        out.push_str("\n  ");
    }
    out.push_str("],\n");

    out.push_str("  \"verification\": {\n    \"status\": ");
    push_json_string(
        &mut out,
        if output.verification.verified() {
            "verified"
        } else {
            "failed"
        },
    );
    out.push_str(",\n    \"checks\": [");
    for (index, check) in verification_checks(output.verification).iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        push_json_string(&mut out, check);
    }
    out.push_str("]\n  }\n}\n");
    out
}

/// Collects the input records for a Lean run, hashing the bytes on disk.
///
/// The emoji text may be normalized in memory for older files, so hashing the in-memory text
/// would make the manifest depend on that normalization; the on-disk bytes are the actual
/// inputs a rebuild needs.
fn lean_records(
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
        records.push(record(path, &bytes, data_dir));
    }
    Ok(records)
}

/// The manifest names of the round-trip checks a run performed.
fn verification_checks(verification: Verification) -> Vec<&'static str> {
    [
        (verification.names, "name-stream-round-trip"),
        (verification.emoji, "emoji-stream-round-trip"),
        (verification.gc, "general-category-round-trip"),
        (verification.patterns, "pattern-round-trip"),
        (verification.image, "image-layout"),
    ]
    .into_iter()
    .filter_map(|(passed, name)| passed.then_some(name))
    .collect()
}

/// One input file recorded in the manifest.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct InputRecord {
    /// The file name as written by Unicode.
    name: String,
    /// The path relative to the data directory, with `/` separators.
    path: String,
    /// The input's byte length.
    size: usize,
    /// The input's FNV-1a 64 hash.
    hash: u64,
}

/// Collects every file that shaped the build, hashing the bytes on disk.
fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            character if character < '\u{20}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

/// Builds one manifest record from a resolved path and its contents.
fn record(path: &Path, bytes: &[u8], data_dir: &Path) -> InputRecord {
    InputRecord {
        name: path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        path: relative_path(path, data_dir),
        size: bytes.len(),
        hash: fnv1a64(bytes),
    }
}

/// Renders `path` relative to `data_dir`, with `/` separators and no absolute components.
fn relative_path(path: &Path, data_dir: &Path) -> String {
    let relative = if data_dir.as_os_str().is_empty() {
        path.file_name().map_or(path, Path::new)
    } else {
        path.strip_prefix(data_dir)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
            .or_else(|| path.file_name().map(Path::new))
            .unwrap_or(path)
    };
    relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}
