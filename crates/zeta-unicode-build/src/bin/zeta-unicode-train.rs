//! Command-line front end for the Nomina trainer.
//!
//! The trainer reads the vendored UCD files, measures every requested name-block shift,
//! writes `nomina.bin` and `manifest.json` into the output directory, and prints the
//! candidate table. Arguments are parsed with `argh`; because `argh` parses `&str`, a
//! non-UTF-8 argument is reported as a usage error instead of panicking.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use argh::FromArgs;
use zeta_unicode_build::{
    DEFAULT_MAX_TABLE_BYTES, MAX_CANDIDATE_SHIFT, MIN_CANDIDATE_SHIFT, Priority, Profile,
    TrainOutput, read_inputs, train, verify, write_manifest_path,
};

/// The temporary image name the trainer stages before publishing `nomina.bin`.
const IMAGE_STAGING_NAME: &str = "nomina.bin.staging";
/// The temporary manifest name, staged so both artifacts publish together.
const MANIFEST_STAGING_NAME: &str = "manifest.json.staging";

/// Trains a Nomina Unicode image from the vendored UCD files.
#[derive(Debug, FromArgs)]
#[argh(help_triggers("-h", "--help"))]
struct Options {
    /// UCD directory (default: $ZETA_UNICODE_DATA_DIR, then ../zeta-unicode/data,
    /// crates/zeta-unicode/data or data)
    #[allow(
        clippy::doc_markdown,
        reason = "the doc comment is the --help text, so backticks would render literally"
    )]
    #[argh(option)]
    data_dir: Option<PathBuf>,
    /// output directory (default: target/nomina)
    #[argh(option, default = "PathBuf::from(\"target/nomina\")")]
    out_dir: PathBuf,
    /// fix the name block shift (3..=8) instead of searching
    #[argh(option, from_str_fn(parse_block_shift))]
    block_shift: Option<u8>,
    /// size, balanced (default) or latency
    #[argh(option, default = "Priority::Balanced")]
    priority: Priority,
    /// image budget in bytes (default: 200000)
    #[argh(option, default = "DEFAULT_MAX_TABLE_BYTES")]
    max_bytes: usize,
    /// compare the trained image against an existing image
    #[argh(option)]
    baseline: Option<PathBuf>,
    /// tolerated growth over the baseline in bytes (default: 0)
    #[argh(option, long = "max-delta", default = "0")]
    max_delta_bytes: usize,
    /// print per-section and per-stream details
    #[argh(switch)]
    report: bool,
    /// verify the trained image before publishing nomina.bin and manifest.json
    #[argh(switch)]
    verify: bool,
}

impl Options {
    /// Rejects flag combinations that `argh` cannot express.
    fn validate(&self) -> Result<(), String> {
        if self.baseline.is_none() && self.max_delta_bytes != 0 {
            return Err("--max-delta needs --baseline to compare against".into());
        }
        Ok(())
    }
}

fn main() -> ExitCode {
    let (program, args) = match utf8_args(std::env::args_os()) {
        Ok(parts) => parts,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let command = command_name(&program);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let options = match Options::from_args(&[command], &args) {
        Ok(options) => options,
        Err(early_exit) if early_exit.status.is_ok() => {
            println!("{}", early_exit.output);
            return ExitCode::SUCCESS;
        }
        Err(early_exit) => {
            eprintln!(
                "{}\n\nRun {command} --help for more information.",
                early_exit.output
            );
            return ExitCode::from(2);
        }
    };
    if let Err(error) = options.validate() {
        eprintln!("error: {error}\n\nRun {command} --help for more information.");
        return ExitCode::from(2);
    }
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Converts the raw arguments to UTF-8 and separates the program name.
///
/// `argh` parses `&str`, so a non-UTF-8 value is a usage error; reporting it here keeps the
/// CLI from panicking inside `std::env::args()`.
fn utf8_args(args: impl IntoIterator<Item = OsString>) -> Result<(String, Vec<String>), String> {
    let mut args = args.into_iter();
    let program = args
        .next()
        .ok_or_else(|| "argv is empty".to_owned())?
        .into_string()
        .map_err(|arg| {
            format!(
                "the program name is not valid UTF-8: {:?}",
                arg.to_string_lossy()
            )
        })?;
    let values = args
        .map(|arg| {
            arg.into_string()
                .map_err(|arg| format!("argument is not valid UTF-8: {:?}", arg.to_string_lossy()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((program, values))
}

/// The program name used in help and usage messages.
fn command_name(program: &str) -> &str {
    Path::new(program)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(program)
}

/// Parses and range-checks the fixed name-block shift for `--block-shift`.
fn parse_block_shift(value: &str) -> Result<u8, String> {
    let shift: u8 = value
        .parse()
        .map_err(|_| format!("expected a byte, got {value:?}"))?;
    if !(MIN_CANDIDATE_SHIFT..=MAX_CANDIDATE_SHIFT).contains(&shift) {
        return Err(format!(
            "must be between {MIN_CANDIDATE_SHIFT} and {MAX_CANDIDATE_SHIFT}, got {shift}"
        ));
    }
    Ok(shift)
}

/// The UCD directory used when `--data-dir` is absent.
fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ZETA_UNICODE_DATA_DIR") {
        return PathBuf::from(dir);
    }
    ["../zeta-unicode/data", "crates/zeta-unicode/data", "data"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_dir())
        .unwrap_or_else(|| PathBuf::from("../zeta-unicode/data"))
}

/// Reads the inputs, trains an image and writes the image and manifest.
fn run(options: &Options) -> Result<(), String> {
    let data_dir = options.data_dir.clone().unwrap_or_else(default_data_dir);
    let inputs = read_inputs(&data_dir).map_err(|error| {
        format!(
            "failed to read the UCD inputs from {}: {error}",
            data_dir.display()
        )
    })?;

    let mut profile = Profile::new(options.priority, options.max_bytes);
    if let Some(shift) = options.block_shift {
        profile = profile.with_block_shift(shift);
    }
    if let Some(path) = &options.baseline {
        let baseline = std::fs::read(path)
            .map_err(|error| format!("failed to read baseline {}: {error}", path.display()))?;
        profile = profile.with_baseline(baseline, options.max_delta_bytes);
    } else {
        profile = profile.with_max_delta_bytes(options.max_delta_bytes);
    }

    let output = train(&inputs, &profile).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&options.out_dir).map_err(|error| {
        format!(
            "failed to create output directory {}: {error}",
            options.out_dir.display()
        )
    })?;
    let image_path = options.out_dir.join("nomina.bin");
    let manifest_path = options.out_dir.join("manifest.json");
    let image_staging = options.out_dir.join(IMAGE_STAGING_NAME);
    let manifest_staging = options.out_dir.join(MANIFEST_STAGING_NAME);

    // Stage the image first: a failed write must not touch the published pair.
    std::fs::write(&image_staging, &output.image)
        .map_err(|error| format!("failed to write {}: {error}", image_staging.display()))?;
    let layout = if options.verify {
        // Verify the staged bytes, not the in-memory buffer: the flag exists to check the
        // bytes a rebuild would actually load.
        let verified = std::fs::read(&image_staging)
            .map_err(|error| {
                let _ = std::fs::remove_file(&image_staging);
                format!("failed to read back {}: {error}", image_staging.display())
            })
            .and_then(|written| {
                verify(&written, &inputs).map_err(|error| {
                    let _ = std::fs::remove_file(&image_staging);
                    error.to_string()
                })
            });
        Some(verified?)
    } else {
        None
    };
    // Stage the manifest beside the image. A failure here leaves the previous image and
    // manifest paired, instead of publishing a new image beside a stale manifest.
    if let Err(error) = write_manifest_path(&manifest_staging, &data_dir, &inputs, &output) {
        let _ = std::fs::remove_file(&image_staging);
        let _ = std::fs::remove_file(&manifest_staging);
        return Err(error.to_string());
    }
    // Publish both: the renames run back to back after every fallible step has succeeded.
    if let Err(error) = std::fs::rename(&image_staging, &image_path) {
        let _ = std::fs::remove_file(&image_staging);
        let _ = std::fs::remove_file(&manifest_staging);
        return Err(format!(
            "failed to publish {}: {error}",
            image_path.display()
        ));
    }
    if let Err(error) = std::fs::rename(&manifest_staging, &manifest_path) {
        return Err(format!(
            "published {} but failed to publish {}: {error}",
            image_path.display(),
            manifest_path.display()
        ));
    }

    print!("{}", output.report);
    if options.report {
        print_details(&output);
    }
    if let Some(layout) = layout {
        println!(
            "verified {} on disk: {} B, {} sections, Unicode {}.{}.{}, name block shift {}",
            image_path.display(),
            layout.total_bytes,
            layout.sections.len(),
            layout.version.0,
            layout.version.1,
            layout.version.2,
            layout.name_block_shift
        );
    }
    println!(
        "wrote {} ({} B) and {}",
        image_path.display(),
        output.total_bytes,
        manifest_path.display()
    );
    Ok(())
}

/// Prints the per-section and per-stream breakdown behind `--report`.
fn print_details(output: &TrainOutput) {
    println!("\nsections:");
    for section in &output.sections {
        println!(
            "  {:<16} offset {:>7}  size {:>7}",
            section.name, section.offset, section.size
        );
    }
    println!(
        "names: {} entries, {} blocks, {} runs, {} contexts",
        output.names.name_count,
        output.names.block_count,
        output.names.run_count,
        output.names.context_count
    );
    println!(
        "  index {} B, offsets {} B, models {} B (directory {} B), stream {} B ({} bits)",
        output.names.index_bytes,
        output.names.offset_bytes,
        output.names.model_bytes,
        output.names.model_directory_bytes,
        output.names.stream_bytes,
        output.names.stream_bits
    );
    println!(
        "emoji: {} entries ({} ruled, {} literal), section {} B",
        output.emoji.entries, output.emoji.ruled, output.emoji.literals, output.emoji.section_bytes
    );
    println!(
        "  keys {} B ({} raw), names {} B ({} raw), rules {} B ({} raw), masks {} B ({} raw)",
        output.emoji.key_stream,
        output.emoji.key_raw,
        output.emoji.name_stream,
        output.emoji.name_raw,
        output.emoji.rule_stream,
        output.emoji.rule_raw,
        output.emoji.mask_stream,
        output.emoji.mask_raw
    );
    println!(
        "general category: {} runs, section {} B",
        output.gc.runs, output.gc.section_bytes
    );
    println!(
        "  starts {} B ({} raw), categories {} B ({} ids), heap {} B",
        output.gc.coded_starts,
        output.gc.starts_raw,
        output.gc.cats_stream,
        output.gc.cats_raw,
        output.gc.heap_bytes
    );
    println!(
        "patterns: {} ranges, section {} B",
        output.patterns.patterns, output.patterns.section_bytes
    );
    println!(
        "  dictionaries {} B, records {} B",
        output.patterns.dictionary_bytes, output.patterns.records_bytes
    );
    if let Some(baseline) = output.baseline {
        println!(
            "baseline: {} B, trained {} B, allowed growth {} B",
            baseline.baseline_bytes, baseline.total_bytes, baseline.max_delta_bytes
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses a list of arguments.
    fn parse(args: &[&str]) -> Result<Options, argh::EarlyExit> {
        Options::from_args(&["zeta-unicode-train"], args)
    }

    #[test]
    fn defaults_match_the_help_text() {
        let options = parse(&[]).expect("defaults parse");
        assert!(options.data_dir.is_none());
        assert_eq!(options.out_dir, PathBuf::from("target/nomina"));
        assert!(options.block_shift.is_none());
        assert_eq!(options.priority, Priority::Balanced);
        assert_eq!(options.max_bytes, DEFAULT_MAX_TABLE_BYTES);
        assert!(options.baseline.is_none());
        assert_eq!(options.max_delta_bytes, 0);
        assert!(!options.report);
        assert!(!options.verify);
    }

    #[test]
    fn parses_every_flag() {
        let options = parse(&[
            "--data-dir",
            "data",
            "--out-dir",
            "out",
            "--block-shift",
            "7",
            "--priority",
            "latency",
            "--max-bytes",
            "1234",
            "--baseline",
            "base.bin",
            "--max-delta",
            "12",
            "--report",
            "--verify",
        ])
        .expect("every flag parses");
        assert_eq!(options.data_dir, Some(PathBuf::from("data")));
        assert_eq!(options.out_dir, PathBuf::from("out"));
        assert_eq!(options.block_shift, Some(7));
        assert_eq!(options.priority, Priority::Latency);
        assert_eq!(options.max_bytes, 1_234);
        assert_eq!(options.baseline, Some(PathBuf::from("base.bin")));
        assert_eq!(options.max_delta_bytes, 12);
        assert!(options.report);
        assert!(options.verify);
    }

    #[test]
    fn priority_names_are_case_insensitive() {
        assert_eq!(
            parse(&["--priority", "SIZE"])
                .expect("upper case priority")
                .priority,
            Priority::Size
        );
    }

    #[test]
    fn rejects_malformed_or_missing_values() {
        for args in [
            ["--block-shift", "2"],
            ["--block-shift", "9"],
            ["--block-shift", "x"],
            ["--priority", "fast"],
            ["--max-bytes", "-1"],
            ["--max-delta", "many"],
        ] {
            assert!(parse(&args).is_err(), "{args:?} must be rejected");
        }
        for args in [
            ["--data-dir"],
            ["--out-dir"],
            ["--block-shift"],
            ["--priority"],
            ["--max-bytes"],
            ["--baseline"],
            ["--max-delta"],
        ] {
            assert!(parse(&args).is_err(), "{args:?} needs a value");
        }
        assert!(parse(&["--unknown"]).is_err());
    }

    #[test]
    fn max_delta_needs_a_baseline() {
        assert!(
            parse(&["--max-delta", "12"])
                .expect("the flags parse")
                .validate()
                .is_err()
        );
        assert!(
            parse(&["--baseline", "base.bin", "--max-delta", "12"])
                .expect("the flags parse")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn help_exits_early_with_a_success_status() {
        let early_exit = parse(&["--help"]).expect_err("--help exits early");
        assert!(early_exit.status.is_ok());
        assert!(early_exit.output.contains("--data-dir"));

        let early_exit = parse(&["-h"]).expect_err("-h exits early");
        assert!(early_exit.status.is_ok());
    }

    #[test]
    fn command_name_strips_the_directory() {
        assert_eq!(
            command_name("/tmp/bin/zeta-unicode-train"),
            "zeta-unicode-train"
        );
        assert_eq!(command_name("zeta-unicode-train"), "zeta-unicode-train");
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_arguments_are_reported_without_panicking() {
        use std::os::unix::ffi::OsStringExt;

        let error = utf8_args([
            OsString::from("zeta-unicode-train"),
            OsString::from("--data-dir"),
            OsString::from_vec(b"data\xFE".to_vec()),
        ])
        .expect_err("non-UTF-8 arguments are rejected");
        assert!(
            error.contains("argument is not valid UTF-8"),
            "unexpected error: {error}"
        );
    }
}
