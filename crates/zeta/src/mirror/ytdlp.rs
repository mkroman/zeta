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
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
};
use tracing::{debug, warn};

use futures::future::BoxFuture;

/// The default command used to run `yt-dlp`.
const DEFAULT_COMMAND: &str = "yt-dlp";

/// The environment variable providing the `yt-dlp` command when unconfigured.
const COMMAND_ENV: &str = "ZETA_YTDLP_COMMAND";

/// The download format — browser-compatible h264 video with the best available audio, falling back
/// to the best available format overall.
const FORMAT: &str = "bestvideo*[vcodec=h264]+bestaudio*/(bv*+ba/b)";

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

/// Errors that can occur while downloading with `yt-dlp`.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// An I/O error occurred.
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    /// The download exceeded the configured timeout.
    #[error("yt-dlp download timed out")]
    Timeout,
    /// `yt-dlp` exited with a failure.
    #[error("yt-dlp failed: {0}")]
    Failure(String),
    /// `yt-dlp` did not report a json dump.
    #[error("yt-dlp did not report a json dump")]
    NoJsonDump,
    /// `yt-dlp` reported no downloaded files.
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
    /// The maximum size of a video to download.
    max_filesize: String,
    /// The maximum duration of a download before it gets killed.
    download_timeout: Duration,
}

/// Options for constructing a [`YtDlp`] runner.
#[derive(Clone, Debug)]
pub struct YtDlpOptions {
    /// The command to execute.
    ///
    /// Falls back to the `ZETA_YTDLP_COMMAND` environment variable, and to `yt-dlp` when
    /// neither is set.
    pub command: Option<String>,
    /// The maximum size of a video to download, as passed to `--max-filesize`.
    pub max_filesize: String,
    /// The maximum duration of a download before it gets killed.
    pub download_timeout: Duration,
}

impl YtDlp {
    /// Creates a new runner from the given options.
    #[must_use]
    pub fn new(options: YtDlpOptions) -> Self {
        Self {
            command: crate::utils::resolve_setting(
                options.command.as_deref(),
                COMMAND_ENV,
                DEFAULT_COMMAND,
            ),
            max_filesize: options.max_filesize,
            download_timeout: options.download_timeout,
        }
    }

    /// Downloads the video at `url` into `output_dir`, naming the downloaded files after `id`,
    /// and streaming progress updates to `on_progress` as they are reported by `yt-dlp`.
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
        id: &str,
        output_dir: &Path,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<Vec<DownloadedFile>, Error> {
        let mut command = Command::new(&self.command);

        command
            .args(build_args(url, id, output_dir, &self.max_filesize))
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

        let output = tokio::time::timeout(self.download_timeout, async {
            let json = read_output(stdout, &mut on_progress).await?;
            let status = child.wait().await?;

            Ok::<RawOutput, Error>(RawOutput { status, json })
        })
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

        let downloads = verify_paths(
            output.json.ok_or(Error::NoJsonDump)?.requested_downloads,
            output_dir,
        )
        .await?;

        if downloads.is_empty() {
            return Err(Error::NoDownloads);
        }

        Ok(downloads)
    }
}

/// Runs downloads on behalf of the download manager, in the shape of the [`YtDlp`] runner.
///
/// This is the seam the download manager's tests substitute a fake into, so they can drive the
/// failure, success and ordering paths without shelling out to a `yt-dlp` stand-in.
pub trait Downloader: Send + Sync + 'static {
    /// Downloads the media at `url` into `output_dir`, naming the files after `id` and streaming
    /// progress updates to `on_progress`.
    ///
    /// The returned future borrows the arguments and the progress callback, so the caller keeps
    /// them alive until the download finishes.
    fn download<'a>(
        &'a self,
        url: &'a str,
        id: &'a str,
        output_dir: &'a Path,
        on_progress: Box<dyn FnMut(Progress) + Send + 'a>,
    ) -> BoxFuture<'a, Result<Vec<DownloadedFile>, Error>>;
}

impl Downloader for YtDlp {
    fn download<'a>(
        &'a self,
        url: &'a str,
        id: &'a str,
        output_dir: &'a Path,
        on_progress: Box<dyn FnMut(Progress) + Send + 'a>,
    ) -> BoxFuture<'a, Result<Vec<DownloadedFile>, Error>> {
        Box::pin(YtDlp::download_with_progress(
            self,
            url,
            id,
            output_dir,
            on_progress,
        ))
    }
}

/// The raw output of a completed `yt-dlp` run.
struct RawOutput {
    /// The exit status of the `yt-dlp` process.
    status: ExitStatus,
    /// The single-video json dump, if one was reported.
    json: Option<JsonDump>,
}

/// Reads the piped stdout of a `yt-dlp` child until EOF, forwarding progress lines to
/// `on_progress` and capturing the json dump.
async fn read_output(
    stdout: impl AsyncRead + Unpin,
    on_progress: &mut impl FnMut(Progress),
) -> Result<Option<JsonDump>, Error> {
    let mut lines = BufReader::new(stdout).lines();
    let mut json = None;

    while let Some(line) = lines.next_line().await? {
        if let Some(progress) = parse_progress_line(&line) {
            on_progress(progress);
        } else if let Ok(dump) = serde_json::from_str::<JsonDump>(&line) {
            json = Some(dump);
        }
    }

    Ok(json)
}

/// Drains the given stderr stream into a string, line by line.
async fn drain_stderr(stderr: impl AsyncRead + Unpin) -> String {
    let mut stderr_text = String::new();
    let mut lines = BufReader::new(stderr).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        stderr_text.push_str(&line);
        stderr_text.push('\n');
    }

    stderr_text
}

/// Builds the `yt-dlp` arguments for downloading `url` into `output_dir`, naming the downloaded
/// files after `id` (i.e. `<id>.<ext>`).
///
/// Configuration files and plugins are disabled, the download is bounded to a single video of at
/// most `max_filesize`, and the URL is passed after a `--` separator so it can never be
/// interpreted as an option.
#[must_use]
fn build_args(url: &str, id: &str, output_dir: &Path, max_filesize: &str) -> Vec<OsString> {
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

/// Returns the downloads whose reported file paths resolve inside `output_dir`, dropping any that
/// point outside of it.
async fn verify_paths(
    downloads: Vec<DownloadedFile>,
    output_dir: &Path,
) -> Result<Vec<DownloadedFile>, Error> {
    let base = tokio::fs::canonicalize(output_dir)
        .await
        .map_err(Error::Io)?;
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

    impl YtDlp {
        /// Creates a runner that invokes the given command.
        #[must_use]
        pub(crate) fn with_command(command: &str) -> Self {
            Self {
                command: command.to_string(),
                max_filesize: "500M".to_string(),
                download_timeout: Duration::from_mins(10),
            }
        }
    }

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
        assert_eq!(output.requested_downloads, Vec::new());
    }

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

    #[tokio::test]
    async fn read_output_streams_progress_and_captures_the_json_dump() {
        let stdout = &b"zeta-dl 512 1024 NA 256 2\n\
                        a line that is neither progress nor json\n\
                        zeta-dl 1024 1024 NA 256 0\n\
                        {\"id\": \"123\", \"requested_downloads\": [{\"filepath\": \"/downloads/123.mp4\", \"id\": \"123\", \"ext\": \"mp4\", \"vcodec\": \"avc1.640029\", \"acodec\": \"mp4a.40.2\"}]}\n"[..];

        let mut progress_updates = Vec::new();

        let json = read_output(stdout, &mut |progress| progress_updates.push(progress))
            .await
            .expect("reading stdout should not fail");

        // The progress lines were streamed to the callback, in order, and other output was
        // ignored.
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

        // The json dump was captured.
        let download = json
            .expect("the json dump should be captured")
            .requested_downloads
            .pop()
            .expect("the json dump should report the download");

        assert_eq!(download.filename().as_deref(), Some("123.mp4"));
        assert!(!download.is_unsupported_codec());
    }

    #[tokio::test]
    async fn verify_paths_keeps_files_inside_the_output_directory() {
        let output_dir = tempfile::tempdir().unwrap();
        let inside = output_dir.path().join("123.mp4");
        std::fs::write(&inside, b"junk").unwrap();

        // A reported absolute path inside the output directory is kept as reported.
        let absolute = DownloadedFile {
            filepath: inside.clone(),
            vcodec: Some("avc1.640029".to_string()),
        };
        // A reported relative path resolves against the output directory.
        let relative = DownloadedFile {
            filepath: PathBuf::from("123.mp4"),
            vcodec: None,
        };
        // A reported path pointing outside of the output directory is dropped.
        let outside = DownloadedFile {
            filepath: PathBuf::from("/etc/passwd"),
            vcodec: None,
        };

        let verified = verify_paths(vec![absolute, relative, outside], output_dir.path())
            .await
            .unwrap();

        assert_eq!(
            verified,
            vec![
                DownloadedFile {
                    filepath: inside,
                    vcodec: Some("avc1.640029".to_string()),
                },
                DownloadedFile {
                    filepath: PathBuf::from("123.mp4"),
                    vcodec: None,
                },
            ]
        );
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
