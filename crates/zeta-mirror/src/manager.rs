//! Download manager for `yt-dlp`.
//!
//! A long-running task that owns the full lifecycle of a mirror: download requests are submitted
//! through a bounded mpsc channel, every accepted request runs as its own `yt-dlp` task that
//! logs its progress to its own span and reports only its final result to the manager over a
//! second mpsc channel, and completed downloads are handed to a bounded upload pipeline that
//! uploads them to S3 before the requester is notified with the public link.
//!
//! At most the configured number of downloads run at a time; further requests are queued and
//! started in order as running downloads finish. A download's slot is released before its files
//! are uploaded, so uploads never keep the download queue from draining and the manager never
//! awaits an upload itself.
//!
//! The accepted-request queue is bounded by [`MAX_QUEUED_REQUESTS`]: a request that arrives while
//! the queue is full is rejected with [`Error::Busy`] instead of being retained, so a stalled
//! upload pipeline cannot grow the manager's in-memory queue without bound. The intake channel
//! itself is bounded as well, so a flood that outruns the manager is shed by the submitter
//! instead of accumulating in the channel.
//!
//! Uploads are bounded explicitly: at most [`UPLOAD_CONCURRENCY`] run at once and at most
//! [`UPLOAD_BACKLOG`] completed downloads wait for a slot. When the backlog is full, no new
//! download is started until an upload finishes; requests keep being accepted and queued, so the
//! manager stays responsive without letting finished downloads accumulate without bound. The
//! downloads that were already running when the backlog filled can still complete, so the queue
//! may briefly exceed its cap by at most their number.
//!
//! The module is split by concern: [`download`] runs `yt-dlp` tasks and reports their final
//! statuses, [`upload`] owns the bounded upload pipeline and the request completion that
//! removes each temporary download directory, and this module owns the intake channel, the
//! coordinator loop and the state the two sides share.

mod download;
#[cfg(test)]
mod tests;
mod upload;

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
};

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{debug, warn};
use url::Url;

use crate::{
    s3::{self, S3},
    ytdlp::{self, DownloadedFile, Downloader},
};

use download::ActiveDownload;
use upload::Upload;

/// The maximum number of uploads that run at the same time.
const UPLOAD_CONCURRENCY: usize = 2;

/// The maximum number of completed downloads waiting for an upload slot.
///
/// Together with [`UPLOAD_CONCURRENCY`] this bounds the temporary directories held by finished
/// downloads. Downloads already running when the queue fills may still complete and add to it, so
/// the queue can exceed this by at most `max_concurrent`.
const UPLOAD_BACKLOG: usize = 8;

/// The maximum number of accepted download requests waiting for a download slot and upload
/// capacity.
///
/// A request that arrives while the queue is full is rejected with [`Error::Busy`]. Without this
/// bound a stalled upload pipeline would let the queue grow with every request for as long as the
/// stall lasts.
const MAX_QUEUED_REQUESTS: usize = 64;

/// Errors that can occur while mirroring a download request.
#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    /// A temporary download directory could not be created.
    #[error("could not create temporary directory: {0}")]
    TempDir(#[from] std::io::Error),
    /// The download failed.
    #[error("download error: {0}")]
    Download(#[from] ytdlp::Error),
    /// The upload of a downloaded file failed.
    #[error("upload error: {0}")]
    Upload(#[from] s3::Error),
    /// The upload task was cancelled or panicked before it could report a result.
    #[error("upload was cancelled")]
    UploadCancelled,
    /// The download id is not safe to use in file names and object keys.
    #[error("invalid download id: {0}")]
    InvalidId(String),
    /// The accepted-request queue is full.
    #[error("download queue is full")]
    Busy,
}

/// Flattens a blocking task's join result into the task's own I/O result.
///
/// A join error — the blocking task panicked or was cancelled — becomes an [`std::io::Error`], so
/// callers handle both failure modes in one place. After a join error the temporary directory may
/// or may not exist, but the request cannot proceed either way.
fn flatten_join<T>(
    result: Result<std::io::Result<T>, tokio::task::JoinError>,
) -> std::io::Result<T> {
    match result {
        Ok(result) => result,
        Err(err) => Err(std::io::Error::other(err)),
    }
}

/// A request to download the media at `url` and mirror it to the bucket.
pub(crate) struct DownloadRequest {
    /// The URL to download.
    pub url: String,
    /// The id the downloaded files are named after and the public link is built from.
    pub id: String,
    /// The key prefix the files are uploaded under.
    pub prefix: String,
    /// The base URL the public link is built from, with the id as its fragment.
    pub public_url_base: Url,
    /// Called exactly once when the download finishes, successfully or not. On success it
    /// receives the public URL of the mirrored media.
    pub on_finish: OnFinish,
    /// The download's `media.download` span, opened when the request was created — while the
    /// requester's span was current — so it nests under the requester. It covers the queue
    /// wait, the download and the upload.
    pub span: tracing::Span,
}

impl std::fmt::Debug for DownloadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadRequest")
            .field("url", &self.url)
            .field("id", &self.id)
            .field("prefix", &self.prefix)
            .field("public_url_base", &self.public_url_base)
            .finish_non_exhaustive()
    }
}

/// The callback invoked when a download request finishes.
type OnFinish = Box<dyn FnOnce(Result<String, Error>) + Send>;

/// A final status sent from a download task to the manager.
///
/// Progress does not cross this channel: it is per-line detail that the download task logs
/// inside its own span, so the manager only ever sees the outcome.
#[derive(Debug)]
enum DownloadStatus {
    /// The download finished and the files are ready for upload.
    Completed { id: u64, files: Vec<DownloadedFile> },
    /// The download failed.
    Failed { id: u64, error: ytdlp::Error },
}

/// The capacity of the intake channel.
///
/// One slot beyond [`MAX_QUEUED_REQUESTS`] lets the manager keep accepting while its accepted
/// queue is exactly full; anything past that is shed before it reaches the manager.
const REQUEST_CHANNEL_CAPACITY: usize = MAX_QUEUED_REQUESTS + 1;

/// A handle to the running download manager, used for submitting download requests.
#[derive(Clone)]
pub(crate) struct DownloadManager {
    /// The channel on which download requests are submitted to the manager task.
    requests: mpsc::Sender<DownloadRequest>,
}

impl DownloadManager {
    /// Starts the download manager task and returns a handle for submitting requests to it.
    ///
    /// At most `max_concurrent` downloads run at a time; a zero is clamped to one so the
    /// manager can always make progress. Downloads are buffered in temporary directories inside
    /// `download_dir`; `base_dir`, when given, is kept alive for as long as the manager runs
    /// (used for a per-run random download directory). Stale temporary directories are removed
    /// at startup; the download directory is expected to be owned by this process and must not
    /// be shared between concurrently running instances.
    ///
    /// Completed downloads are uploaded by a bounded pipeline: at most [`UPLOAD_CONCURRENCY`]
    /// uploads run at once and at most [`UPLOAD_BACKLOG`] wait. When the backlog is full the
    /// manager stops starting downloads until an upload finishes, so finished media cannot pile
    /// up without bound. At most [`MAX_QUEUED_REQUESTS`] accepted requests wait for a download
    /// slot; further requests are rejected with [`Error::Busy`].
    ///
    /// The manager task itself has no span: its lifetime is the process, not a unit of work, so a
    /// span for it would never be exported and would outlive whatever started it. Each download,
    /// together with the upload that finishes it, runs in a `media.download` span instead.
    ///
    /// # Panics
    ///
    /// Panics if called outside of a tokio runtime.
    #[must_use]
    pub(crate) fn start(
        downloader: Arc<dyn Downloader>,
        s3: S3,
        download_dir: PathBuf,
        base_dir: Option<tempfile::TempDir>,
        max_concurrent: usize,
    ) -> Self {
        let removed = super::tempdir::cleanup_stale_downloads(&download_dir, None);
        if removed > 0 {
            debug!(
                media.download.stale_removed.count = removed,
                "cleaned up stale download directories"
            );
        }

        if max_concurrent == 0 {
            warn!("max_concurrent is 0; running one download at a time");
        }

        let max_concurrent = max_concurrent.max(1);
        let (requests, request_rx) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
        let (status_tx, status_rx) = mpsc::unbounded_channel();

        let manager = Manager {
            downloader,
            s3,
            download_dir,
            _base_dir: base_dir,
            max_concurrent,
            requests: request_rx,
            status_tx,
            status: status_rx,
            active: HashMap::new(),
            queue: VecDeque::new(),
            uploads: JoinSet::new(),
            upload_queue: VecDeque::new(),
            next_id: 0,
            queue_full_warned: false,
        };

        tokio::spawn(manager.run());

        Self { requests }
    }

    /// Submits a download request to the manager.
    ///
    /// The request is handled asynchronously; its [`DownloadRequest::on_finish`] callback is
    /// invoked when the download finishes, or immediately with [`Error::Busy`] when the intake
    /// channel is full. On that path the callback runs on the submitting thread.
    ///
    /// # Errors
    ///
    /// Returns the request back if the manager task is no longer running.
    #[allow(clippy::result_large_err)]
    pub(crate) fn submit(&self, request: DownloadRequest) -> Result<(), DownloadRequest> {
        match self.requests.try_send(request) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(request)) => {
                (request.on_finish)(Err(Error::Busy));

                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(request)) => Err(request),
        }
    }
}

/// The state of the download manager task.
struct Manager {
    /// The downloader used for downloading media.
    downloader: Arc<dyn Downloader>,
    /// The S3 client used for uploading media.
    s3: S3,
    /// The directory that downloads are buffered in.
    download_dir: PathBuf,
    /// A random download directory, kept alive for as long as the manager runs.
    _base_dir: Option<tempfile::TempDir>,
    /// The maximum number of downloads that may run concurrently.
    max_concurrent: usize,
    /// The channel download requests arrive on.
    requests: mpsc::Receiver<DownloadRequest>,
    /// The channel used for sending statuses to newly spawned download tasks.
    status_tx: mpsc::UnboundedSender<DownloadStatus>,
    /// The channel download tasks report their status on.
    status: mpsc::UnboundedReceiver<DownloadStatus>,
    /// The downloads that are currently running, by id.
    active: HashMap<u64, ActiveDownload>,
    /// The requests waiting for a download slot.
    queue: VecDeque<DownloadRequest>,
    /// The uploads currently running; completed uploads are reaped from the join set.
    uploads: JoinSet<()>,
    /// Completed downloads waiting for an upload slot, in completion order.
    upload_queue: VecDeque<Upload>,
    /// The id to assign to the next download.
    next_id: u64,
    /// Whether the current full-queue episode has already produced its warning.
    ///
    /// A sustained outage rejects many requests; logging every rejection at `warn` would flood
    /// the log. The first rejection warns, subsequent ones are debug-level, and the flag clears
    /// once the queue drains so the next episode warns again.
    queue_full_warned: bool,
}

impl Manager {
    /// Runs the manager until either of its channels closes.
    async fn run(mut self) {
        debug!("download manager started");

        loop {
            // Computed outside the `select!` so the `join_next` branch can be disabled without
            // borrowing the join set twice; an empty `JoinSet` always resolves immediately.
            let has_uploads = !self.uploads.is_empty();

            tokio::select! {
                request = self.requests.recv() => {
                    let Some(request) = request else { break };
                    self.on_request(request).await;
                }
                status = self.status.recv() => {
                    // The sending end lives in `self`, so the status channel cannot close
                    // while the manager runs; the match only makes the select total.
                    let Some(status) = status else { break };
                    self.on_status(status).await;
                }
                upload = self.uploads.join_next(), if has_uploads => {
                    self.on_upload_finished(upload).await;
                }
            }
        }

        // Completed downloads that never started their upload still own a callback and a
        // temporary directory; dropping them runs the completion. Running uploads are aborted
        // and drained so their completion runs too, instead of waiting for the process to exit.
        self.uploads.abort_all();
        while self.uploads.join_next().await.is_some() {}
        self.upload_queue.clear();

        debug!("download manager stopped");
    }

    /// Handles an incoming download request, starting it or queueing it for later.
    ///
    /// A request that arrives while [`MAX_QUEUED_REQUESTS`] are already waiting is rejected with
    /// [`Error::Busy`] instead of growing the queue, so a stalled upload pipeline cannot retain
    /// accepted requests without bound.
    async fn on_request(&mut self, request: DownloadRequest) {
        if self.active.len() < self.max_concurrent && !self.upload_backlog_full() {
            self.start_download(request).await;
        } else if self.queue.len() >= MAX_QUEUED_REQUESTS {
            if self.queue_full_warned {
                debug!(
                    media.id = %request.id,
                    media.download.queued.count = self.queue.len(),
                    "rejecting download request, the queue is full"
                );
            } else {
                warn!(
                    media.id = %request.id,
                    media.download.queued.count = self.queue.len(),
                    "download queue is full; shedding load until a slot frees"
                );

                self.queue_full_warned = true;
            }

            (request.on_finish)(Err(Error::Busy));
        } else {
            debug!(
                media.id = %request.id,
                media.download.running.count = self.active.len(),
                media.upload.running.count = self.uploads.len(),
                media.upload.waiting.count = self.upload_queue.len(),
                "queued download request"
            );

            self.queue.push_back(request);
        }
    }
}
