//! The download side of the manager: running `yt-dlp` tasks and reporting their final status.
//!
//! A started download owns the request's temporary directory and its `media.download` span until
//! it reports a status. The manager then either queues the completed files for upload or removes
//! the directory and reports the failure; request admission and the concurrency limit live in
//! the parent module.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{Instrument, debug, error, trace, warn};

use crate::tempdir::tempdir_builder;
use crate::ytdlp::{self, DownloadedFile, Progress};

use super::upload::UploadCompletion;
use super::{DownloadRequest, DownloadStatus, Error, Manager, flatten_join};

/// A running download and the state the manager keeps around it.
pub(super) struct ActiveDownload {
    /// The request the download was started for.
    pub(super) request: DownloadRequest,
    /// The temporary directory the media is downloaded into, removed when the download finishes.
    pub(super) tempdir: tempfile::TempDir,
    /// The download's `media.download` span, re-entered while the manager handles the download's
    /// failure and by the upload task that carries its completed files, so those log lines and
    /// S3 spans stay in its unit of work.
    pub(super) span: tracing::Span,
}

impl Manager {
    /// Starts a download task for the given request, creating its temporary download directory on
    /// the blocking pool.
    ///
    /// The directory exists before the download task is spawned, and a failure to create it is
    /// reported to the requester instead of starting the download.
    pub(super) async fn start_download(&mut self, request: DownloadRequest) {
        if !zeta_url::is_identifier(&request.id, "_-") {
            warn!(media.id = %request.id, "rejecting download with an unsafe id");

            (request.on_finish)(Err(Error::InvalidId(request.id)));

            return;
        }

        let id = self.next_id;
        self.next_id += 1;

        let download_dir = self.download_dir.clone();
        let tempdir = match flatten_join(
            tokio::task::spawn_blocking(move || tempdir_builder().tempdir_in(&download_dir)).await,
        ) {
            Ok(tempdir) => tempdir,
            Err(err) => {
                error!(
                    media.id = %request.id,
                    exception.message = %err,
                    "could not create a temporary download directory"
                );

                (request.on_finish)(Err(Error::TempDir(err)));

                return;
            }
        };

        let downloader = Arc::clone(&self.downloader);
        let task = DownloadTask::new(id, self.status_tx.clone());
        let url = request.url.clone();
        let media_id = request.id.clone();
        let path = tempdir.path().to_path_buf();

        // The span was opened when the request was queued, so it covers the wait as well as the
        // download and the upload. Re-entering it here keeps the pre-start diagnostics in it;
        // the spawned download task and the later upload task re-enter it for the rest of the
        // unit of work.
        let span = request.span.clone();

        span.in_scope(|| {
            debug!(
                zeta.download.id = %id,
                file.path = %path.display(),
                "starting download"
            );
        });

        tokio::spawn(
            async move {
                let result = downloader
                    .download(&url, &media_id, &path, Box::new(DownloadTask::progress))
                    .await;

                match result {
                    Ok(files) => task.completed(files),
                    Err(error) => task.failed(error),
                }
            }
            .instrument(span.clone()),
        );

        self.active.insert(
            id,
            ActiveDownload {
                request,
                tempdir,
                span,
            },
        );
    }

    /// Handles a final status reported by a download task.
    ///
    /// The download's slot is released before its files are uploaded. A completed download is
    /// only queued for the upload pipeline — never uploaded here — so a stalled or slow upload
    /// cannot stop the manager from accepting requests or handling the other downloads'
    /// statuses. A failed download has nothing to upload and is reported inline after its
    /// temporary directory is removed, as before.
    ///
    /// The upload task re-enters the finished download's `media.download` span, while starting
    /// the next download happens outside it, so that download remains the root of its own trace
    /// instead of becoming a child of the download that freed the slot.
    pub(super) async fn on_status(&mut self, status: DownloadStatus) {
        let (id, result) = match status {
            DownloadStatus::Completed { id, files } => (id, Ok(files)),
            DownloadStatus::Failed { id, error } => (id, Err(error)),
        };

        let Some(active) = self.active.remove(&id) else {
            return;
        };

        match result {
            Ok(files) => {
                // Queueing first lets the backpressure check in `start_next` see the finished
                // download: the slot is free, but the upload pipeline may not be.
                self.enqueue_upload(active, files);
                self.start_next().await;
            }
            Err(error) => {
                let span = active.span.clone();

                span.in_scope(|| {
                    warn!(
                        exception.message = %error,
                        "could not download media"
                    );
                });

                self.start_next().await;
                Self::fail_download(active, error).await;
            }
        }
    }

    /// Reports a failed download after removing its temporary directory on the blocking pool.
    ///
    /// A failed download has nothing to upload, so its callback runs inline once the removal has
    /// completed; the slot has already been released by the caller, keeping the queue draining.
    async fn fail_download(active: ActiveDownload, error: ytdlp::Error) {
        let ActiveDownload {
            request: DownloadRequest { on_finish, .. },
            tempdir,
            ..
        } = active;

        UploadCompletion::new(on_finish, tempdir)
            .complete(Err(Error::Download(error)))
            .await;
    }
}

/// Sends status updates from a download task to the manager.
///
/// If the task panics or is cancelled before reporting a result, dropping the guard reports the
/// download as failed, so that the manager releases its slot and cleans up.
struct DownloadTask {
    /// The id of the download.
    id: u64,
    /// The channel to send status updates on, taken when a final status is reported.
    status_tx: Option<mpsc::UnboundedSender<DownloadStatus>>,
}

impl DownloadTask {
    const fn new(id: u64, status_tx: mpsc::UnboundedSender<DownloadStatus>) -> Self {
        Self {
            id,
            status_tx: Some(status_tx),
        }
    }

    /// Logs a progress update of the download.
    ///
    /// Progress is per-line detail, so it is logged by the download task — inside its
    /// `media.download` span — instead of crossing the manager's status channel.
    fn progress(progress: Progress) {
        trace!(
            media.download.progress_percent = progress.fraction().map(|fraction| fraction * 100.0),
            media.download.progress = ?progress,
            "download progress"
        );
    }

    /// Reports the download as completed, disarming the guard.
    fn completed(mut self, files: Vec<DownloadedFile>) {
        self.send(DownloadStatus::Completed { id: self.id, files });
        self.status_tx = None;
    }

    /// Reports the download as failed, disarming the guard.
    fn failed(mut self, error: ytdlp::Error) {
        self.send(DownloadStatus::Failed { id: self.id, error });
        self.status_tx = None;
    }

    /// Sends a status update to the manager, ignoring a closed channel.
    fn send(&self, status: DownloadStatus) {
        if let Some(status_tx) = &self.status_tx {
            let _ = status_tx.send(status);
        }
    }
}

impl Drop for DownloadTask {
    fn drop(&mut self) {
        self.send(DownloadStatus::Failed {
            id: self.id,
            error: ytdlp::Error::TaskCancelled,
        });
    }
}
