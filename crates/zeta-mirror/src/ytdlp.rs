//! Integration with `yt-dlp` for downloading videos.
//!
//! Downloads stream their progress: `yt-dlp` is run with a custom `--progress-template` and the
//! output is read line-by-line, so a caller can observe the state of the download as it happens.
//!
//! The module is split by concern: [`args`] builds the command line, [`process`] runs the child,
//! streams its output and verifies the downloaded files, and [`progress`] parses the progress
//! lines it emits.

mod args;
mod process;
mod progress;
#[cfg(test)]
mod tests;

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use futures::future::BoxFuture;
use serde::Deserialize;

pub use progress::Progress;

/// The default command used to run `yt-dlp`.
const DEFAULT_COMMAND: &str = "yt-dlp";

/// The environment variable providing the `yt-dlp` command when unconfigured.
const COMMAND_ENV: &str = "ZETA_YTDLP_COMMAND";

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
    /// neither is set. Empty values count as unset.
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
            command: zeta_utils::settings::resolve_optional_setting(
                options.command.as_deref(),
                COMMAND_ENV,
            )
            .unwrap_or_else(|| DEFAULT_COMMAND.to_owned()),
            max_filesize: options.max_filesize,
            download_timeout: options.download_timeout,
        }
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
        Box::pin(Self::download_with_progress(
            self,
            url,
            id,
            output_dir,
            on_progress,
        ))
    }
}
