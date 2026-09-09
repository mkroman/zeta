//! Integration with `yt-dlp` for downloading videos.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::Deserialize;
use tokio::process::Command;
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

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("yt-dlp download timed out")]
    Timeout,
    #[error("yt-dlp failed: {0}")]
    Failure(String),
    #[error("could not parse yt-dlp output: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("yt-dlp reported no downloaded files")]
    NoDownloads,
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

    /// Downloads the video at `url` into `output_dir` and returns the downloaded files.
    ///
    /// Videos are downloaded in a browser-compatible h264 format and merged into an mp4 container.
    /// Reported files that resolve outside of `output_dir` are dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if `yt-dlp` could not be spawned, exits with a failure, times out or
    /// produces output that can't be parsed.
    pub async fn download(&self, url: &str, output_dir: &Path) -> Result<Vec<DownloadedFile>, Error> {
        let mut command = Command::new(&self.command);

        command
            .args(build_args(url, output_dir))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Ensure the child gets killed if we're dropped (e.g. by a timeout).
            .kill_on_drop(true);

        debug!(%url, command = ?command.as_std(), "downloading video with yt-dlp");

        let child = command.spawn().map_err(Error::Io)?;

        let output = tokio::time::timeout(DOWNLOAD_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(Error::Io)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Failure(truncate_tail(stderr.trim_end())));
        }

        let output: JsonDump = serde_json::from_slice(&output.stdout)?;
        let downloads = verify_paths(output.requested_downloads, output_dir).await?;

        if downloads.is_empty() {
            return Err(Error::NoDownloads);
        }

        Ok(downloads)
    }
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
    fn test_truncate_tail() {
        assert_eq!(truncate_tail("hello"), "hello");
        assert_eq!(truncate_tail("æøå"), "æøå");

        let long = "a".repeat(STDERR_MESSAGE_LENGTH + 20);
        let truncated = truncate_tail(&long);
        assert_eq!(truncated.chars().count(), STDERR_MESSAGE_LENGTH);
    }
}
