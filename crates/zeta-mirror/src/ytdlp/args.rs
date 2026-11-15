//! Construction of the `yt-dlp` command line.
//!
//! The arguments harden the run — no configuration files or plugins, a single video of bounded
//! size — name the downloaded files after the media id and force the progress template that
//! [`super::progress`] parses.

use std::{ffi::OsString, path::Path};

/// The download format — browser-compatible h264 video with the best available audio, falling back
/// to the best available format overall.
const FORMAT: &str = "bestvideo*[vcodec=h264]+bestaudio*/(bv*+ba/b)";

/// The template passed to `--progress-template`, emitting raw numeric progress fields (or `NA`
/// when a field is unknown) on a single line per update.
const PROGRESS_TEMPLATE: &str = "download:zeta-dl %(progress.downloaded_bytes)s \
     %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s \
     %(progress.eta)s";

/// Builds the `yt-dlp` arguments for downloading `url` into `output_dir`, naming the downloaded
/// files after `id` (i.e. `<id>.<ext>`).
///
/// Configuration files and plugins are disabled, the download is bounded to a single video of at
/// most `max_filesize`, and the URL is passed after a `--` separator so it can never be
/// interpreted as an option.
#[must_use]
pub(super) fn build_args(
    url: &str,
    id: &str,
    output_dir: &Path,
    max_filesize: &str,
) -> Vec<OsString> {
    vec![
        "--ignore-config".into(),
        "--no-plugin-dirs".into(),
        "--no-playlist".into(),
        // Force progress output even when stdout is not a terminal, with one line per update, and
        // a machine-readable template that we can distinguish from the json dump.
        "--progress".into(),
        "--newline".into(),
        "--progress-template".into(),
        PROGRESS_TEMPLATE.into(),
        "--output".into(),
        format!("{id}.%(ext)s").into(),
        "--paths".into(),
        output_dir.as_os_str().to_os_string(),
        "--dump-single-json".into(),
        "--no-simulate".into(),
        "--max-filesize".into(),
        max_filesize.into(),
        "--format".into(),
        FORMAT.into(),
        "--merge-output-format".into(),
        "mp4".into(),
        "--".into(),
        url.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_args() {
        let args: Vec<String> = build_args(
            "https://www.tiktok.com/@a/video/1",
            "123",
            Path::new("/tmp/out"),
            "500M",
        )
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

        let value = |option: &str| {
            args.iter()
                .position(|arg| arg == option)
                .map(|index| args[index + 1].as_str())
        };

        // The URL is passed after the `--` separator, so it can never be interpreted as an option.
        let separator = args
            .iter()
            .position(|arg| arg == "--")
            .expect("missing `--` separator");
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://www.tiktok.com/@a/video/1")
        );
        assert!(separator < args.len() - 1);

        // Hardening options are present.
        for option in ["--ignore-config", "--no-plugin-dirs", "--no-playlist"] {
            assert!(args.contains(&option.to_string()), "missing {option}");
        }

        // Progress is forced on (stdout is a pipe, so it would be disabled otherwise) and emitted
        // with one line per update in our machine-readable template.
        for option in ["--progress", "--newline", "--progress-template"] {
            assert!(args.contains(&option.to_string()), "missing {option}");
        }

        let template = value("--progress-template").expect("missing progress template");
        assert!(template.starts_with("download:zeta-dl "));
        assert!(template.contains("%(progress.downloaded_bytes)s"));
        assert!(template.contains("%(progress.total_bytes)s"));
        assert!(template.contains("%(progress.eta)s"));

        // The downloaded files are named after the given id.
        assert_eq!(value("--output"), Some("123.%(ext)s"));
        // The output directory is passed with `--paths` rather than changing the working directory.
        assert_eq!(value("--paths"), Some("/tmp/out"));
        // The format selector has a fallback.
        assert_eq!(
            value("--format"),
            Some("bestvideo*[vcodec=h264]+bestaudio*/(bv*+ba/b)")
        );
        // The download size is capped.
        assert_eq!(value("--max-filesize"), Some("500M"));
    }
}
