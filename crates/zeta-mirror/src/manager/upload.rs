//! The upload side of the manager: the bounded S3 upload pipeline and request completion.
//!
//! Completed downloads enter a backlog as [`Upload`]s; at most [`UPLOAD_CONCURRENCY`] run at once
//! and at most [`UPLOAD_BACKLOG`] wait for a slot. Every upload ends through [`UploadCompletion`],
//! which removes the temporary download directory and reports the requester exactly once, even
//! when its task is cancelled or panics.

use tracing::{Instrument, debug, error, warn};
use url::Url;

use crate::s3::{self, S3};
use crate::ytdlp::DownloadedFile;
use crate::{object_key, public_url_for};

use super::download::ActiveDownload;
use super::{
    DownloadRequest, Error, Manager, OnFinish, UPLOAD_BACKLOG, UPLOAD_CONCURRENCY, flatten_join,
};

/// A completed download whose files are waiting for, or running through, their upload.
pub(super) struct Upload {
    /// The id the files are named after and the public link is built from.
    id: String,
    /// The key prefix the files are uploaded under.
    prefix: String,
    /// The base URL the public link is built from, with the id as its fragment.
    public_url_base: Url,
    /// The downloaded files to upload.
    files: Vec<DownloadedFile>,
    /// The callback and temporary directory, torn down exactly once whichever way the upload ends.
    completion: UploadCompletion,
    /// The download's `media.download` span, kept for the upload's logs and S3 spans.
    span: tracing::Span,
}

impl Upload {
    /// Uploads the files to the bucket and reports the result to the requester.
    async fn run(self, s3: S3) {
        let Self {
            id,
            prefix,
            public_url_base,
            files,
            completion,
            ..
        } = self;

        let result = match Manager::upload(&s3, &prefix, &public_url_base, &id, &files).await {
            Ok(link) => Ok(link),
            Err(err) => {
                error!(
                    exception.message = %err,
                    "could not upload the downloaded media"
                );
                Err(Error::Upload(err))
            }
        };

        completion.complete(result).await;
    }
}

/// Removes an upload's temporary directory and notifies its requester exactly once.
///
/// Completion removes the directory on the blocking pool and only then invokes the callback, so
/// the callback never observes a directory that is still being removed. The completion is owned
/// by the [`Upload`] (or by its running task), so dropping it without completing it — because the
/// task was cancelled or panicked — still runs the callback and removes the files synchronously.
pub(super) struct UploadCompletion {
    /// The requester callback, taken when it is invoked so it can never run twice.
    on_finish: Option<OnFinish>,
    /// The temporary download directory, taken when it is removed.
    tempdir: Option<tempfile::TempDir>,
}

impl UploadCompletion {
    /// Creates the completion state for an upload.
    pub(super) fn new(on_finish: OnFinish, tempdir: tempfile::TempDir) -> Self {
        Self {
            on_finish: Some(on_finish),
            tempdir: Some(tempdir),
        }
    }

    /// Removes the temporary directory on the blocking pool and reports `result` to the
    /// requester.
    ///
    /// A cleanup failure is logged but does not change the result the requester receives.
    pub(super) async fn complete(mut self, result: Result<String, Error>) {
        self.close_tempdir_async().await;

        if let Some(on_finish) = self.on_finish.take() {
            on_finish(result);
        }
    }

    /// Removes the temporary directory on the blocking pool, if it has not been removed yet.
    ///
    /// A failure is logged; it does not change the primary result reported to the requester.
    async fn close_tempdir_async(&mut self) {
        let Some(tempdir) = self.tempdir.take() else {
            return;
        };

        // The path is captured before `close`, since it is no longer valid afterwards.
        let tempdir_path = tempdir.path().to_path_buf();

        match flatten_join(tokio::task::spawn_blocking(move || tempdir.close()).await) {
            Ok(()) => {}
            Err(err) => {
                warn!(
                    file.path = %tempdir_path.display(),
                    exception.message = %err,
                    "could not remove the temporary download directory"
                );
            }
        }
    }

    /// Removes the temporary directory synchronously, if it has not been removed yet.
    ///
    /// This is the fallback for a cancelled or panicking upload task, where there is no async
    /// context left to await the blocking pool.
    fn close_tempdir(&mut self) {
        let Some(tempdir) = self.tempdir.take() else {
            return;
        };

        // The path is captured before `close`, since it is no longer valid afterwards.
        let tempdir_path = tempdir.path().to_path_buf();

        if let Err(err) = tempdir.close() {
            warn!(
                file.path = %tempdir_path.display(),
                exception.message = %err,
                "could not remove the temporary download directory"
            );
        }
    }
}

impl Drop for UploadCompletion {
    fn drop(&mut self) {
        self.close_tempdir();

        if let Some(on_finish) = self.on_finish.take() {
            on_finish(Err(Error::UploadCancelled));
        }
    }
}

impl Manager {
    /// Queues a completed download for upload and starts as many queued uploads as the
    /// pipeline allows.
    pub(super) fn enqueue_upload(&mut self, active: ActiveDownload, files: Vec<DownloadedFile>) {
        let ActiveDownload {
            request:
                DownloadRequest {
                    id,
                    prefix,
                    public_url_base,
                    on_finish,
                    ..
                },
            tempdir,
            span,
        } = active;

        self.upload_queue.push_back(Upload {
            id,
            prefix,
            public_url_base,
            files,
            completion: UploadCompletion::new(on_finish, tempdir),
            span,
        });

        self.pump_uploads();
    }

    /// Starts queued uploads until the upload pipeline is full or the queue is empty.
    fn pump_uploads(&mut self) {
        while self.uploads.len() < UPLOAD_CONCURRENCY {
            let Some(upload) = self.upload_queue.pop_front() else {
                break;
            };

            self.start_upload(upload);
        }
    }

    /// Spawns the task that uploads `upload` inside its download's `media.download` span.
    fn start_upload(&mut self, upload: Upload) {
        let s3 = self.s3.clone();
        let span = upload.span.clone();

        self.uploads.spawn(
            async move {
                upload.run(s3).await;
            }
            .instrument(span),
        );
    }

    /// Handles a finished upload task, then refills the upload pipeline and the download slots
    /// the completed upload may have unblocked.
    pub(super) async fn on_upload_finished(
        &mut self,
        joined: Option<Result<(), tokio::task::JoinError>>,
    ) {
        if let Some(Err(error)) = joined {
            // The upload's completion guard has already reported the cancellation or panic.
            if error.is_panic() {
                error!(exception.message = %error, "upload task panicked");
            } else {
                warn!(exception.message = %error, "upload task was cancelled");
            }
        }

        self.pump_uploads();
        self.start_next().await;
    }

    /// Returns whether the upload pipeline has no room for another completed download.
    ///
    /// A queued upload is drained into a free slot immediately, so a non-empty queue means every
    /// upload slot is busy. Downloads already running when this becomes true may still finish
    /// and queue their uploads; their number is bounded by `max_concurrent`.
    // `VecDeque::len` is not const on stable, so a `const fn` would not compile there.
    #[allow(
        clippy::missing_const_for_fn,
        reason = "VecDeque::len is not const on stable"
    )]
    pub(super) fn upload_backlog_full(&self) -> bool {
        self.upload_queue.len() >= UPLOAD_BACKLOG
    }

    /// Uploads the downloaded files to the bucket under `prefix` and returns the public URL of
    /// the mirrored media.
    ///
    /// Files that already exist in the bucket under their exact key are not uploaded again.
    ///
    /// # Errors
    ///
    /// Returns an error if the presence of a file could not be checked or a file could not be
    /// uploaded.
    async fn upload(
        s3: &S3,
        prefix: &str,
        public_url_base: &Url,
        id: &str,
        files: &[DownloadedFile],
    ) -> Result<String, s3::Error> {
        for file in files {
            if file.is_unsupported_codec() {
                warn!(
                    media.video.codec = ?file.vcodec,
                    "the media is in a format that isn't supported by browsers"
                );
            }

            let Some(filename) = file.filename() else {
                continue;
            };

            let key = object_key(prefix, &filename);

            if s3.object_exists(&key).await? {
                debug!(aws.s3.key = %key, "skipping upload, object already exists");
                continue;
            }

            s3.upload_file(&file.filepath, &key).await?;
            debug!(aws.s3.key = %key, "uploaded media to s3");
        }

        Ok(public_url_for(public_url_base, id))
    }

    /// Starts queued downloads until every slot is busy, the upload backlog is full or the
    /// queue is empty.
    ///
    /// A request that [`start_download`](Self::start_download) rejects early — an unsafe id or a
    /// temporary directory that could not be created — never becomes active, so draining the
    /// queue here keeps a rejected request from stranding every request behind it.
    pub(super) async fn start_next(&mut self) {
        while self.active.len() < self.max_concurrent && !self.upload_backlog_full() {
            let Some(request) = self.queue.pop_front() else {
                break;
            };

            self.start_download(request).await;
        }

        if self.queue.is_empty() {
            self.queue_full_warned = false;
        }
    }
}
