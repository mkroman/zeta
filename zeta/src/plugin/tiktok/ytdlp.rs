//! Integration with `yt-dlp` for downloading videos.
//!
//! Downloads stream their progress: `yt-dlp` is run with a custom `--progress-template` and the
//! output is read line-by-line, so a caller can observe the state of the download as it happens.

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};

use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStdout, Command},
};
use tracing::{debug, warn};

/// The default command used to run `yt-dlp`.
const DEFAULT_COMMAND: &str = "yt-dlp";

/// The output filename template, resulting in `<video id>.<ext>`.
const OUTPUT_TEMPLATE: &str = "%(id)s.%(ext)s";

/// The download format — browser-compatible h264 video with the best available audio, falling back
/// to the best available format overall.
const FORMAT: &str = "bestvideo*[vcodec=h264]+bestaudio*/(bv*+ba/b)";

/// The maximum size of a video to download, as passed to `--max-filesize`. Videos that report a
/// larger size upfront are skipped; sizes are not always known in advance, so the download timeout
/// remains the final bound.
const MAX_FILESIZE: &str = "500M";

/// The maximum duration of a download before it gets killed.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(10);

/// The maximum number of characters to include of `yt-dlp`'s stderr in error messages.
const STDERR_MESSAGE_LENGTH: usize = 300;

/// The marker prefixing the progress lines emitted with [`PROGRESS_TEMPLATE`], used to tell them
/// apart from `yt-dlp`'s other output.
const PROGRESS_PREFIX: &str = "zeta-dl";

/// The template passed to `--progress-template`, emitting raw numeric progress fields (or `NA`
/// when a field is unknown) on a single line per update.
const PROGRESS_TEMPLATE: &str = "download:zeta-dl %(progress.downloaded_bytes)s \
     %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s \
     %(progress.eta)s";

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    #[error("yt-dlp download timed out")]
    Timeout,
    #[error("yt-dlp failed: {0}")]
    Failure(String),
    #[error("yt-dlp did not report a json dump")]
    NoJsonDump,
    #[error("yt-dlp reported no downloaded files")]
    NoDownloads,
    /// The task running the download was cancelled or panicked before it could report a result.
    #[error("download task was cancelled or panicked")]
    TaskCancelled,
}

/// A progress update of a running download, as reported by `yt-dlp`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Progress {
    /// The number of bytes downloaded so far.
    pub downloaded: Option<u64>,
    /// The total size of the download in bytes, if known or estimated upfront.
    pub total: Option<u64>,
    /// The current download speed in bytes per second.
    pub speed: Option<u64>,
    /// The estimated time remaining in seconds.
    pub eta: Option<u64>,
}

impl Progress {
    /// Returns the downloaded fraction of the total size, if the total size is known.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn fraction(&self) -> Option<f64> {
        let total = self.total? as f64;
        let downloaded = self.downloaded.unwrap_or_default() as f64;

        (total > 0.0).then_some(downloaded / total)
    }
}

/// Parses a progress line emitted with [`PROGRESS_TEMPLATE`], returning `None` for any other line.
fn parse_progress_line(line: &str) -> Option<Progress> {
    let fields = line.strip_prefix(PROGRESS_PREFIX)?.split_ascii_whitespace();

    let mut fields = fields.map(parse_size_field);
    let downloaded = fields.next()?;
    let total = fields.next()?;
    let total_estimate = fields.next()?;
    let speed = fields.next()?;
    let eta = fields.next()?;

    Some(Progress {
        downloaded,
        // The exact size is unknown ("NA") for some downloads; fall back to the estimate.
        total: total.or(total_estimate),
        speed,
        eta,
    })
}

/// Parses a raw `yt-dlp` progress field, returning `None` if it is `NA` or not a number.
fn parse_size_field(field: &str) -> Option<u64> {
    field.parse().ok()
}

/// A downloaded file as reported by `yt-dlp`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct DownloadedFile {
    /// The path of the downloaded file.
    pub filepath: PathBuf,
    /// The video codec of the downloaded file, if known.
    pub vcodec: Option<String>,
}

impl DownloadedFile {
    /// Returns the name of the downloaded file.
    #[must_use]
    pub fn filename(&self) -> Option<String> {
        self.filepath
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .map(ToOwned::to_owned)
    }

    /// Returns whether the video codec is unsupported by browsers (i.e. not h264).
    #[must_use]
    pub fn is_unsupported_codec(&self) -> bool {
        self.vcodec
            .as_deref()
            .is_some_and(|vcodec| vcodec != "h264" && !vcodec.starts_with("avc1"))
    }
}

#[derive(Debug, Deserialize)]
struct JsonDump {
    #[serde(default)]
    requested_downloads: Vec<DownloadedFile>,
}

/// A runner for `yt-dlp`.
#[derive(Clone, Debug)]
pub struct YtDlp {
    /// The command to execute.
    command: String,
}

impl YtDlp {
    /// Creates a new runner, using the path to the `yt-dlp` binary from `TIKTOK_YTDLP_COMMAND` or
    /// the default.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            command: std::env::var("TIKTOK_YTDLP_COMMAND")
                .unwrap_or_else(|_| DEFAULT_COMMAND.to_string()),
        }
    }

    /// Creates a runner that invokes the given command.
    #[cfg(test)]
    #[must_use]
    pub fn with_command(command: &str) -> Self {
        Self {
            command: command.to_string(),
        }
    }

    /// Downloads the video at `url` into `output_dir`, streaming progress updates to
    /// `on_progress` as they are reported by `yt-dlp`.
    ///
    /// Videos are downloaded in a browser-compatible h264 format and merged into an mp4 container.
    /// Reported files that resolve outside of `output_dir` are dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if `yt-dlp` could not be spawned, exits with a failure, times out or
    /// produces output that can't be parsed.
    pub async fn download_with_progress(
        &self,
        url: &str,
        output_dir: &Path,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<Vec<DownloadedFile>, Error> {
        let mut command = Command::new(&self.command);

        command
            .args(build_args(url, output_dir))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Ensure the child gets killed if we're dropped (e.g. by a timeout).
            .kill_on_drop(true);

        debug!(%url, command = ?command.as_std(), "downloading video with yt-dlp");

        let mut child = command.spawn().map_err(Error::Io)?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Io(io::Error::other("yt-dlp stdout is not piped")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Io(io::Error::other("yt-dlp stderr is not piped")))?;

        // Drain stderr in a separate task so it can never fill up its pipe and block the child.
        let stderr_task = tokio::spawn(drain_stderr(stderr));

        let output = tokio::time::timeout(
            DOWNLOAD_TIMEOUT,
            read_output(&mut child, stdout, &mut on_progress),
        )
        .await
        .map_err(|_| {
            stderr_task.abort();
            Error::Timeout
        })??;

        if !output.status.success() {
            // The child has exited, so its stderr will reach EOF shortly.
            let stderr = stderr_task.await.unwrap_or_default();

            return Err(Error::Failure(truncate_tail(stderr.trim_end())));
        }

        stderr_task.abort();

        let downloads = verify_paths(output.json.ok_or(Error::NoJsonDump)?.requested_downloads, output_dir).await?;

        if downloads.is_empty() {
            return Err(Error::NoDownloads);
        }

        Ok(downloads)
    }
}

/// The raw output of a completed `yt-dlp` run.
struct RawOutput {
    /// The exit status of the `yt-dlp` process.
    status: ExitStatus,
    /// The single-video json dump, if one was reported.
    json: Option<JsonDump>,
}

/// Reads the piped stdout of the given `yt-dlp` child until EOF, forwarding progress lines to
/// `on_progress` and capturing the json dump, then waits for the child to exit.
async fn read_output(
    child: &mut Child,
    stdout: ChildStdout,
    on_progress: &mut impl FnMut(Progress),
) -> Result<RawOutput, Error> {
    let mut lines = BufReader::new(stdout).lines();
    let mut json = None;

    while let Some(line) = lines.next_line().await? {
        if let Some(progress) = parse_progress_line(&line) {
            on_progress(progress);
        } else if let Ok(dump) = serde_json::from_str::<JsonDump>(&line) {
            json = Some(dump);
        }
    }

    Ok(RawOutput {
        status: child.wait().await?,
        json,
    })
}

/// Drains the given stderr stream into a string, line by line.
async fn drain_stderr(stderr: impl tokio::io::AsyncRead + Unpin) -> String {
    let mut stderr_text = String::new();
    let mut lines = BufReader::new(stderr).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        stderr_text.push_str(&line);
        stderr_text.push('\n');
    }

    stderr_text
}

/// Builds the `yt-dlp` arguments for downloading `url` into `output_dir`.
///
/// Configuration files and plugins are disabled, the download is bounded to a single video of at
/// most [`MAX_FILESIZE`], and the URL is passed after a `--` separator so it can never be
/// interpreted as an option.
#[must_use]
fn build_args(url: &str, output_dir: &Path) -> Vec<OsString> {
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
        OUTPUT_TEMPLATE.into(),
        "--paths".into(),
        output_dir.as_os_str().to_os_string(),
        "--dump-single-json".into(),
        "--no-simulate".into(),
        "--max-filesize".into(),
        MAX_FILESIZE.into(),
        "--format".into(),
        FORMAT.into(),
        "--merge-output-format".into(),
        "mp4".into(),
        "--".into(),
        url.into(),
    ]
}

/// Returns the downloads whose reported file paths resolve inside `output_dir`, dropping any that
/// point outside of it.
async fn verify_paths(downloads: Vec<DownloadedFile>, output_dir: &Path) -> Result<Vec<DownloadedFile>, Error> {
    let base = tokio::fs::canonicalize(output_dir).await.map_err(Error::Io)?;
    let mut verified = Vec::with_capacity(downloads.len());

    for download in downloads {
        // The reported path is normally absolute; fall back to resolving it relative to the
        // output directory.
        let path = match tokio::fs::canonicalize(&download.filepath).await {
            Ok(path) => Ok(path),
            Err(err) => tokio::fs::canonicalize(output_dir.join(&download.filepath))
                .await
                .map_err(|_| err),
        };

        match path {
            Ok(path) if path.starts_with(&base) => verified.push(download),
            _ => warn!(
                filepath = %download.filepath.display(),
                "ignoring downloaded file outside of the output directory"
            ),
        }
    }

    Ok(verified)
}

/// Returns at most the last [`STDERR_MESSAGE_LENGTH`] characters of `s`.
fn truncate_tail(s: &str) -> String {
    let char_count = s.chars().count();

    if char_count <= STDERR_MESSAGE_LENGTH {
        return s.to_string();
    }

    s.chars().skip(char_count - STDERR_MESSAGE_LENGTH).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_OUTPUT: &str = r#"{
        "id": "7541501431543532814",
        "title": "some video title",
        "requested_downloads": [
            {
                "filepath": "/tmp/example/7541501431543532814.mp4",
                "id": "7541501431543532814",
                "ext": "mp4",
                "vcodec": "avc1.640029",
                "acodec": "mp4a.40.2"
            }
        ]
    }"#;

    #[test]
    fn test_parse_output() {
        // Downloads are reported when present, and absent (not an error) otherwise.
        let output: JsonDump = serde_json::from_str(SAMPLE_OUTPUT).unwrap();
        assert_eq!(output.requested_downloads.len(), 1);

        let download = &output.requested_downloads[0];
        assert_eq!(
            download.filepath,
            PathBuf::from("/tmp/example/7541501431543532814.mp4")
        );
        assert_eq!(
            download.filename().as_deref(),
            Some("7541501431543532814.mp4")
        );
        assert!(!download.is_unsupported_codec());

        let output: JsonDump = serde_json::from_str("{}").unwrap();
        assert!(output.requested_downloads.is_empty());
    }

    #[test]
    fn test_build_args() {
        let args: Vec<String> =
            build_args("https://www.tiktok.com/@a/video/1", Path::new("/tmp/out"))
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

    #[test]
    fn test_parse_progress_line() {
        let progress = parse_progress_line("zeta-dl 512 1024 NA 256 2").unwrap();
        assert_eq!(
            progress,
            Progress {
                downloaded: Some(512),
                total: Some(1024),
                speed: Some(256),
                eta: Some(2),
            }
        );
        assert_eq!(progress.fraction(), Some(0.5));

        // The estimated size is used when the exact size is unknown.
        let progress = parse_progress_line("zeta-dl 100 NA 400 10 30").unwrap();
        assert_eq!(progress.total, Some(400));
        assert_eq!(progress.fraction(), Some(0.25));

        // Fields that are unknown are reported as absent.
        assert_eq!(
            parse_progress_line("zeta-dl NA NA NA NA NA"),
            Some(Progress::default())
        );

        // Anything else is not a progress line.
        assert_eq!(parse_progress_line(r#"{"id": "123"}"#), None);
        assert_eq!(parse_progress_line(PROGRESS_PREFIX), None);
        assert_eq!(parse_progress_line(""), None);
    }

    /// Writes an executable script that acts like a successful `yt-dlp` run: it emits progress
    /// lines, writes a file into the directory passed via `--paths`, and dumps its json.
    fn write_successful_script() -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script =
            std::env::temp_dir().join(format!("zeta-test-ytdlp-{}.sh", std::process::id()));
        std::fs::write(
            &script,
            r#"#!/bin/sh
while [ $# -gt 0 ]; do
  if [ "$1" = "--paths" ] && [ -n "$2" ]; then
    dir="$2"
  fi
  shift
done
echo "zeta-dl 512 1024 NA 256 2"
echo "zeta-dl 1024 1024 NA 256 0"
printf junk > "$dir/123.mp4"
printf '{"id": "123", "requested_downloads": [{"filepath": "%s/123.mp4", "id": "123", "ext": "mp4", "vcodec": "avc1.640029", "acodec": "mp4a.40.2"}]}' "$dir"
"#,
        )
        .unwrap();

        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();

        script
    }

    #[tokio::test]
    async fn test_download_with_progress() {
        let output_dir = tempfile::tempdir().unwrap();
        let script = write_successful_script();
        let ytdlp = YtDlp::with_command(script.to_str().unwrap());

        let mut progress_updates = Vec::new();
        let downloads = ytdlp
            .download_with_progress(
                "https://www.tiktok.com/@user/video/123",
                output_dir.path(),
                |progress| progress_updates.push(progress),
            )
            .await
            .unwrap();

        // The file reported by the json dump is verified and returned.
        assert_eq!(downloads.len(), 1);
        assert_eq!(downloads[0].filename().as_deref(), Some("123.mp4"));

        // The progress lines were streamed to the callback, in order.
        assert_eq!(
            progress_updates,
            vec![
                Progress {
                    downloaded: Some(512),
                    total: Some(1024),
                    speed: Some(256),
                    eta: Some(2),
                },
                Progress {
                    downloaded: Some(1024),
                    total: Some(1024),
                    speed: Some(256),
                    eta: Some(0),
                },
            ]
        );

        std::fs::remove_file(&script).unwrap();
    }

    #[test]
    fn test_truncate_tail() {
        assert_eq!(truncate_tail("hello"), "hello");
        assert_eq!(truncate_tail("æøå"), "æøå");

        let long = "a".repeat(STDERR_MESSAGE_LENGTH + 20);
        let truncated = truncate_tail(&long);
        assert_eq!(truncated.chars().count(), STDERR_MESSAGE_LENGTH);
    }
}
