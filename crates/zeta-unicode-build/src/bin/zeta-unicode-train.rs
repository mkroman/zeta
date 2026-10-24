//! Command-line front end for the Nomina trainer.
//!
//! The trainer reads the vendored UCD files, measures every requested name-block shift,
//! writes `nomina.bin` and `manifest.json` into the output directory, and prints the
//! candidate table. Arguments are parsed by hand so the generator stays dependency-free.

use std::path::PathBuf;
use std::process::ExitCode;

use zeta_unicode_build::{
    DEFAULT_MAX_TABLE_BYTES, MAX_CANDIDATE_SHIFT, MIN_CANDIDATE_SHIFT, Priority, Profile,
    TrainOutput, read_inputs, train, verify, write_manifest,
};

/// The help text printed by `--help` and on a usage error.
const HELP: &str = "\
zeta-unicode-train - train a Nomina Unicode image

Usage: zeta-unicode-train [options]

Options:
  --data-dir <dir>    UCD directory (default: $ZETA_UNICODE_DATA_DIR, then
                      ../zeta-unicode/data, crates/zeta-unicode/data or data)
  --out-dir <dir>     output directory (default: target/nomina)
  --block-shift <n>   fix the name block shift (3..=8) instead of searching
  --priority <mode>   size, balanced (default) or latency
  --max-bytes <n>     image budget in bytes (default: 200000)
  --baseline <table>  compare the trained image against an existing image
  --max-delta <n>     tolerated growth over the baseline in bytes (default: 0)
  --report            print per-section and per-stream details
  --verify            re-verify the trained image and its name section
  -h, --help          print this help
";

/// The parsed command line.
struct Options {
    /// The UCD directory, or `None` to use the platform default.
    data_dir: Option<PathBuf>,
    /// Where the image and manifest are written.
    out_dir: PathBuf,
    /// A fixed name-block shift, or `None` to search.
    block_shift: Option<u8>,
    /// How to choose between candidates.
    priority: Priority,
    /// The image budget.
    max_bytes: usize,
    /// An existing image to compare against.
    baseline: Option<PathBuf>,
    /// The tolerated growth over the baseline.
    max_delta_bytes: usize,
    /// Whether to print per-section details.
    report: bool,
    /// Whether to re-verify the written image.
    verify: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            data_dir: None,
            out_dir: PathBuf::from("target/nomina"),
            block_shift: None,
            priority: Priority::Balanced,
            max_bytes: DEFAULT_MAX_TABLE_BYTES,
            baseline: None,
            max_delta_bytes: 0,
            report: false,
            verify: false,
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let options = match parse_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("error: {error}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Parses every option, rejecting unknown flags and malformed values.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => options.data_dir = Some(PathBuf::from(next_value(&mut args, &arg)?)),
            "--out-dir" => options.out_dir = PathBuf::from(next_value(&mut args, &arg)?),
            "--block-shift" => {
                let value = next_value(&mut args, &arg)?;
                let shift: u8 = value
                    .parse()
                    .map_err(|_| format!("{arg} expects a byte, got {value:?}"))?;
                if !(MIN_CANDIDATE_SHIFT..=MAX_CANDIDATE_SHIFT).contains(&shift) {
                    return Err(format!(
                        "{arg} must be between {MIN_CANDIDATE_SHIFT} and \
                         {MAX_CANDIDATE_SHIFT}, got {shift}"
                    ));
                }
                options.block_shift = Some(shift);
            }
            "--priority" => {
                let value = next_value(&mut args, &arg)?;
                options.priority = Priority::parse(&value).ok_or_else(|| {
                    format!("{arg} expects size, balanced or latency, got {value:?}")
                })?;
            }
            "--max-bytes" => options.max_bytes = parse_usize(&mut args, &arg)?,
            "--baseline" => options.baseline = Some(PathBuf::from(next_value(&mut args, &arg)?)),
            "--max-delta" => options.max_delta_bytes = parse_usize(&mut args, &arg)?,
            "--report" => options.report = true,
            "--verify" => options.verify = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(options)
}

/// Takes the value that follows a flag.
fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} needs a value"))
}

/// Parses the value of a byte-count flag.
fn parse_usize(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<usize, String> {
    let value = next_value(args, flag)?;
    value
        .parse()
        .map_err(|_| format!("{flag} expects a non-negative integer, got {value:?}"))
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
    std::fs::write(&image_path, &output.image)
        .map_err(|error| format!("failed to write {}: {error}", image_path.display()))?;
    write_manifest(&data_dir, &options.out_dir, &inputs, &output)
        .map_err(|error| error.to_string())?;

    print!("{}", output.report);
    if options.report {
        print_details(&output);
    }
    if options.verify {
        let layout = verify(&output.image, &inputs).map_err(|error| error.to_string())?;
        println!(
            "verified: {} B, {} sections, Unicode {}.{}.{}, name block shift {}",
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
        options.out_dir.join("manifest.json").display()
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
        "general category: {} runs, section {} B",
        output.gc.runs, output.gc.section_bytes
    );
    println!(
        "patterns: {} ranges, section {} B",
        output.patterns.patterns, output.patterns.section_bytes
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
    fn parse(args: &[&str]) -> Result<Options, String> {
        parse_args(args.iter().map(ToString::to_string))
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
}
