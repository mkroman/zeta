//! Download manager for `yt-dlp`.
//!
//! A long-running task that owns the full lifecycle of a mirror: download requests are submitted
//! through an unbounded mpsc channel, every accepted request runs as its own `yt-dlp` task that
//! logs its progress to its own span and reports only its final result to the manager over a
//! second mpsc channel, and completed downloads are uploaded to S3 inline before the requester is
//! notified with the public link.
//!
//! At most the configured number of downloads run at a time; further requests are queued and
//! started in order as running downloads finish. A download's slot is released before its files
//! are uploaded, so one upload may overlap the downloads that follow it.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
};

use tokio::sync::mpsc;
use tracing::{Instrument, debug, error, trace, warn};
use url::Url;

use super::{
    object_key, public_url_for,
    s3::S3,
    tempdir::tempdir_builder,
    ytdlp::{self, DownloadedFile, Downloader, Progress},
};
use zeta_url::is_identifier;

/// Errors that can occur while mirroring a download request.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// A temporary download directory could not be created.
    #[error("could not create temporary directory: {0}")]
    TempDir(#[from] std::io::Error),
    /// The download failed.
    #[error("download error: {0}")]
    Download(#[from] ytdlp::Error),
    /// The upload of a downloaded file failed.
    #[error("upload error: {0}")]
    Upload(#[from] super::s3::Error),
    /// The download id is not safe to use in file names and object keys.
    #[error("invalid download id: {0}")]
    InvalidId(String),
}

/// A request to download the media at `url` and mirror it to the bucket.
pub struct DownloadRequest {
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

/// A handle to the running download manager, used for submitting download requests.
#[derive(Clone)]
pub struct DownloadManager {
    /// The channel on which download requests are submitted to the manager task.
    requests: mpsc::UnboundedSender<DownloadRequest>,
}

impl DownloadManager {
    /// Starts the download manager task and returns a handle for submitting requests to it.
    ///
    /// At most `max_concurrent` downloads run at a time. Downloads are buffered in temporary
    /// directories inside `download_dir`; `base_dir`, when given, is kept alive for as long as
    /// the manager runs (used for a per-run random download directory). Stale temporary
    /// directories are removed at startup; the download directory is expected to be owned by
    /// this process and must not be shared between concurrently running instances.
    ///
    /// The manager task itself has no span: its lifetime is the process, not a unit of work, so a
    /// span for it would never be exported and would outlive whatever started it. Each download,
    /// together with the upload that finishes it, runs in a `media.download` span instead.
    ///
    /// # Panics
    ///
    /// Panics if called outside of a tokio runtime.
    #[must_use]
    pub fn start(
        downloader: Arc<dyn Downloader>,
        s3: S3,
        download_dir: PathBuf,
        base_dir: Option<tempfile::TempDir>,
        max_concurrent: usize,
    ) -> Self {
        let removed = super::tempdir::cleanup_stale_downloads(&download_dir, None);
        if removed > 0 {
            debug!(removed, "cleaned up stale download directories");
        }

        let (requests, request_rx) = mpsc::unbounded_channel();
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
            next_id: 0,
        };

        tokio::spawn(manager.run());

        Self { requests }
    }

    /// Submits a download request to the manager.
    ///
    /// The request is handled asynchronously; its [`DownloadRequest::on_finish`] callback is
    /// invoked when the download finishes.
    ///
    /// # Errors
    ///
    /// Returns the request back if the manager task is no longer running.
    #[allow(clippy::result_large_err)]
    pub fn submit(&self, request: DownloadRequest) -> Result<(), DownloadRequest> {
        self.requests.send(request).map_err(|err| err.0)
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
    requests: mpsc::UnboundedReceiver<DownloadRequest>,
    /// The channel used for sending statuses to newly spawned download tasks.
    status_tx: mpsc::UnboundedSender<DownloadStatus>,
    /// The channel download tasks report their status on.
    status: mpsc::UnboundedReceiver<DownloadStatus>,
    /// The downloads that are currently running, by id.
    active: HashMap<u64, ActiveDownload>,
    /// The requests waiting for a download slot.
    queue: VecDeque<DownloadRequest>,
    /// The id to assign to the next download.
    next_id: u64,
}

/// A running download and the state the manager keeps around it.
struct ActiveDownload {
    /// The request the download was started for.
    request: DownloadRequest,
    /// The temporary directory the media is downloaded into, removed when the download finishes.
    tempdir: tempfile::TempDir,
    /// The download's `media.download` span, re-entered while the manager handles the download's
    /// status and uploads its files, so those log lines and S3 spans stay in its unit of work.
    span: tracing::Span,
}

impl Manager {
    /// Runs the manager until either of its channels closes.
    async fn run(mut self) {
        debug!("download manager started");

        loop {
            tokio::select! {
                request = self.requests.recv() => {
                    let Some(request) = request else { break };
                    self.on_request(request);
                }
                status = self.status.recv() => {
                    // The sending end lives in `self`, so the status channel cannot close
                    // while the manager runs; the match only makes the select total.
                    let Some(status) = status else { break };
                    self.on_status(status).await;
                }
            }
        }

        debug!("download manager stopped");
    }

    /// Handles an incoming download request, starting it or queueing it for later.
    fn on_request(&mut self, request: DownloadRequest) {
        if self.active.len() < self.max_concurrent {
            self.start_download(request);
        } else {
            debug!(
                media.id = %request.id,
                running = self.active.len(),
                "queued download request"
            );

            self.queue.push_back(request);
        }
    }

    /// Starts a download task for the given request, creating its temporary download directory.
    fn start_download(&mut self, request: DownloadRequest) {
        if !is_identifier(&request.id, "_-") {
            warn!(media.id = %request.id, "rejecting download with an unsafe id");

            (request.on_finish)(Err(Error::InvalidId(request.id)));

            return;
        }

        let id = self.next_id;
        self.next_id += 1;

        let tempdir = match tempdir_builder().tempdir_in(&self.download_dir) {
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
        // the spawned download task and the manager re-enter it for the rest of the unit of
        // work.
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
    /// The download's slot is released before its files are uploaded: the upload can take a
    /// while, so the next queued download starts right away instead of waiting behind it. The
    /// upload itself still runs inside the finished download's `media.download` span, still
    /// serialized on the manager task, and the next download starts outside that span so it
    /// becomes a root of its own rather than a child of the download that freed the slot. The
    /// cost is that at most `max_concurrent` downloads plus one upload run at a time, i.e. one
    /// extra temporary download directory.
    async fn on_status(&mut self, status: DownloadStatus) {
        let (id, result) = match status {
            DownloadStatus::Completed { id, files } => (id, Ok(files)),
            DownloadStatus::Failed { id, error } => (id, Err(error)),
        };

        let Some(active) = self.active.remove(&id) else {
            return;
        };

        let span = active.span.clone();

        if let Err(error) = &result {
            span.in_scope(|| {
                warn!(
                    exception.message = %error,
                    "could not download media"
                );
            });
        }

        self.start_next();

        Self::finish(&self.s3, active, result)
            .instrument(span)
            .await;
    }

    /// Finishes a download: uploads the downloaded files to the bucket (if any), notifies the
    /// requester of the result and removes the temporary download directory.
    ///
    /// The caller runs this inside the download's `media.download` span.
    async fn finish(
        s3: &S3,
        active: ActiveDownload,
        result: Result<Vec<DownloadedFile>, ytdlp::Error>,
    ) {
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
            ..
        } = active;

        let result = match result {
            Ok(files) => match Self::upload(s3, &prefix, &public_url_base, &id, &files).await {
                Ok(link) => Ok(link),
                Err(err) => {
                    error!(
                        exception.message = %err,
                        "could not upload the downloaded media"
                    );
                    Err(Error::Upload(err))
                }
            },
            Err(err) => Err(Error::Download(err)),
        };

        // The temporary directory and everything in it are removed here, both on success and on
        // failure, so that downloaded files never outlive the download. The path is captured
        // before `close`, since it is no longer valid afterwards.
        let tempdir_path = tempdir.path().to_path_buf();

        if let Err(err) = tempdir.close() {
            warn!(
                file.path = %tempdir_path.display(),
                exception.message = %err,
                "could not remove the temporary download directory"
            );
        }

        on_finish(result);
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
    ) -> Result<String, super::s3::Error> {
        for file in files {
            if file.is_unsupported_codec() {
                warn!(
                    vcodec = ?file.vcodec,
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

    /// Starts queued downloads until every slot is busy or the queue is empty.
    ///
    /// A request that [`start_download`](Self::start_download) rejects early — an unsafe id or a
    /// temporary directory that could not be created — never becomes active, so draining the
    /// queue here keeps a rejected request from stranding every request behind it.
    fn start_next(&mut self) {
        while self.active.len() < self.max_concurrent {
            let Some(request) = self.queue.pop_front() else {
                break;
            };

            self.start_download(request);
        }
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
            percent = progress.fraction().map(|fraction| fraction * 100.0),
            ?progress,
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::Mutex;

    use futures::future::BoxFuture;
    use tokio::sync::oneshot;

    use super::*;
    use crate::tempdir::TEMP_DIR_PREFIX;

    /// Returns the temporary download directories that currently exist under `base`.
    fn stale_download_dirs(base: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(base)
            .unwrap()
            .flatten()
            .filter(crate::tempdir::is_temp_download_dir)
            .map(|entry| entry.path())
            .collect()
    }

    /// Returns a request that reports its result on the given channel.
    fn request(
        url: &str,
        id: &str,
        results: mpsc::UnboundedSender<(usize, Result<String, Error>)>,
        index: usize,
    ) -> DownloadRequest {
        DownloadRequest {
            url: url.to_string(),
            id: id.to_string(),
            prefix: "test".to_string(),
            public_url_base: Url::parse("https://links.example/test").unwrap(),
            on_finish: Box::new(move |result| {
                let _ = results.send((index, result));
            }),
            span: tracing::Span::none(),
        }
    }

    /// Starts a manager with its own isolated download directory, so tests cannot interfere with
    /// each other.
    fn start_manager(
        downloader: Arc<dyn Downloader>,
        s3: S3,
        max_concurrent: usize,
    ) -> (tempfile::TempDir, DownloadManager) {
        let download_dir = tempfile::tempdir().unwrap();
        let manager = DownloadManager::start(
            downloader,
            s3,
            download_dir.path().to_path_buf(),
            None,
            max_concurrent,
        );

        (download_dir, manager)
    }

    /// A downloader fake that fails every download, recording the media ids it was asked to
    /// download, so a test can prove a download never started.
    #[derive(Default)]
    struct RecordingDownloader {
        started: Mutex<Vec<String>>,
    }

    impl Downloader for RecordingDownloader {
        fn download<'a>(
            &'a self,
            _url: &'a str,
            id: &'a str,
            _output_dir: &'a Path,
            _on_progress: Box<dyn FnMut(Progress) + Send + 'a>,
        ) -> BoxFuture<'a, Result<Vec<DownloadedFile>, ytdlp::Error>> {
            self.started.lock().unwrap().push(id.to_string());

            Box::pin(async { Err(ytdlp::Error::NoDownloads) })
        }
    }

    /// A downloader fake that writes a media file into the output directory and reports it, as a
    /// completed `yt-dlp` run would.
    struct CompletingDownloader;

    impl Downloader for CompletingDownloader {
        fn download<'a>(
            &'a self,
            _url: &'a str,
            id: &'a str,
            output_dir: &'a Path,
            mut on_progress: Box<dyn FnMut(Progress) + Send + 'a>,
        ) -> BoxFuture<'a, Result<Vec<DownloadedFile>, ytdlp::Error>> {
            let filepath = output_dir.join(format!("{id}.mp4"));
            std::fs::write(&filepath, b"junk").expect("write the fake download");

            // A progress update flows through the manager before the download completes.
            on_progress(Progress {
                downloaded: Some(1),
                total: Some(4),
                speed: Some(2),
                eta: Some(1),
            });

            let files = vec![DownloadedFile {
                filepath,
                vcodec: Some("avc1.640029".to_string()),
            }];

            Box::pin(async { Ok(files) })
        }
    }

    /// A downloader fake whose downloads block until the test releases them one by one, so
    /// ordering is proven exactly rather than by timing.
    struct GatedDownloader {
        /// The media ids of the downloads as they start, in order.
        started: mpsc::UnboundedSender<String>,
        /// The gate each download waits on, handed out in start order.
        gates: Mutex<VecDeque<oneshot::Receiver<()>>>,
    }

    impl Downloader for GatedDownloader {
        fn download<'a>(
            &'a self,
            _url: &'a str,
            id: &'a str,
            _output_dir: &'a Path,
            _on_progress: Box<dyn FnMut(Progress) + Send + 'a>,
        ) -> BoxFuture<'a, Result<Vec<DownloadedFile>, ytdlp::Error>> {
            let gate = self
                .gates
                .lock()
                .unwrap()
                .pop_front()
                .expect("no download is started without a gate");

            self.started
                .send(id.to_string())
                .expect("the test listens for starts");

            Box::pin(async move {
                gate.await.expect("the gate should not be dropped");

                Err(ytdlp::Error::NoDownloads)
            })
        }
    }

    /// A downloader fake that completes every download except a gated one, so a test can hold
    /// the queue's next download open while the previous one is uploading.
    struct MixedDownloader {
        /// The media ids of the downloads as they start, in order.
        started: mpsc::UnboundedSender<String>,
        /// The gate the gated download waits on.
        gate: Mutex<Option<oneshot::Receiver<()>>>,
    }

    impl Downloader for MixedDownloader {
        fn download<'a>(
            &'a self,
            _url: &'a str,
            id: &'a str,
            output_dir: &'a Path,
            _on_progress: Box<dyn FnMut(Progress) + Send + 'a>,
        ) -> BoxFuture<'a, Result<Vec<DownloadedFile>, ytdlp::Error>> {
            self.started
                .send(id.to_string())
                .expect("the test listens for starts");

            if id == "second" {
                let gate = self
                    .gate
                    .lock()
                    .unwrap()
                    .take()
                    .expect("the second download starts once");

                return Box::pin(async move {
                    gate.await.expect("the gate should not be dropped");

                    Err(ytdlp::Error::NoDownloads)
                });
            }

            let filepath = output_dir.join(format!("{id}.mp4"));
            std::fs::write(&filepath, b"junk").expect("write the fake download");

            let files = vec![DownloadedFile {
                filepath,
                vcodec: None,
            }];

            Box::pin(async { Ok(files) })
        }
    }

    #[tokio::test]
    async fn test_invalid_id_is_rejected() {
        let downloader = Arc::new(RecordingDownloader::default());
        let (_download_dir, manager) = start_manager(downloader.clone(), S3::for_test(), 2);

        let (results, mut rx) = mpsc::unbounded_channel();
        manager
            .submit(request("0", "../evil", results, 0))
            .expect("manager is running");

        let (_, result) = rx.recv().await.expect("the download did not finish");
        let error = result.expect_err("the request should have been rejected");

        assert!(matches!(error, Error::InvalidId(_)));

        // The request was rejected before it reached the downloader.
        zeta_test_support::assert_empty!(downloader.started.lock().unwrap());
    }

    #[tokio::test]
    async fn test_failed_download_reports_and_cleans_up() {
        let (_download_dir, manager) =
            start_manager(Arc::new(RecordingDownloader::default()), S3::for_test(), 2);

        let (results, mut rx) = mpsc::unbounded_channel();
        manager
            .submit(request("0", "123", results, 0))
            .expect("manager is running");

        let (_, result) = rx.recv().await.expect("the download did not finish");
        let error = result.expect_err("the download should have failed");

        assert!(matches!(error, Error::Download(_)));
    }

    #[tokio::test]
    async fn test_failed_download_cleans_up_its_directory() {
        let downloader = Arc::new(RecordingDownloader::default());
        let (download_dir, manager) = start_manager(downloader, S3::for_test(), 2);

        let (results, mut rx) = mpsc::unbounded_channel();
        manager
            .submit(request("0", "123", results, 0))
            .expect("manager is running");

        let (_, _) = rx.recv().await.expect("the download did not finish");

        // The temporary download directory was removed.
        assert!(
            stale_download_dirs(download_dir.path()).is_empty(),
            "temporary download directories were not removed: {:?}",
            stale_download_dirs(download_dir.path())
        );
    }

    #[tokio::test]
    async fn test_completed_download_is_uploaded_and_reported() {
        let server = zeta_test_support::not_found_server().await;
        let s3 = S3::with_endpoint(&format!("http://{}", server.address()));
        let (download_dir, manager) = start_manager(Arc::new(CompletingDownloader), s3, 2);

        let (results, mut rx) = mpsc::unbounded_channel();
        manager
            .submit(request(
                "https://www.tiktok.com/@user/video/123",
                "123",
                results,
                0,
            ))
            .expect("manager is running");

        // The download succeeds, but the upload fails against the always-404 server.
        let (_, result) = rx.recv().await.expect("the download did not finish");
        let error = result.expect_err("the upload should have failed");

        assert!(matches!(error, Error::Upload(_)));

        // The upload checked the object (HEAD, answered as absent) and attempted the upload
        // (PUT) — the only two requests the flow makes.
        let requests = server
            .received_requests()
            .await
            .expect("the server tracks requests");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method.as_str(), "HEAD");
        assert_eq!(requests[1].method.as_str(), "PUT");

        // The temporary download directory was removed.
        assert_eq!(
            stale_download_dirs(download_dir.path()),
            Vec::<PathBuf>::new()
        );
    }

    #[tokio::test]
    async fn test_download_slot_is_released_before_the_upload() {
        use std::time::Duration;

        use zeta_test_support::wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};

        // The presence check of the first download's upload never answers within the test, so
        // only releasing the slot before the upload lets the second download start.
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(404).set_delay(Duration::from_mins(60)))
            .mount(&server)
            .await;

        let s3 = S3::with_endpoint(&format!("http://{}", server.address()));
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (gate, gate_rx) = oneshot::channel();
        let downloader = Arc::new(MixedDownloader {
            started,
            gate: Mutex::new(Some(gate_rx)),
        });
        let (_download_dir, manager) = start_manager(downloader, s3, 1);

        let (results, mut rx) = mpsc::unbounded_channel();
        manager
            .submit(request("0", "first", results.clone(), 0))
            .expect("manager is running");
        manager
            .submit(request("0", "second", results, 1))
            .expect("manager is running");

        assert_eq!(started_rx.recv().await.as_deref(), Some("first"));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
                .await
                .expect("the second download should start while the first is uploading")
                .as_deref(),
            Some("second")
        );

        drop(gate);

        // The first download's callback is still serialized behind its upload.
        assert!(
            rx.try_recv().is_err(),
            "the first request was reported before its upload finished"
        );
    }

    #[tokio::test]
    async fn test_requests_run_serially_when_capped() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (gate_0, gate_rx_0) = oneshot::channel();
        let (gate_1, gate_rx_1) = oneshot::channel();
        let (gate_2, gate_rx_2) = oneshot::channel();

        let downloader = Arc::new(GatedDownloader {
            started,
            gates: Mutex::new(VecDeque::from([gate_rx_0, gate_rx_1, gate_rx_2])),
        });
        let (_download_dir, manager) = start_manager(downloader, S3::for_test(), 1);

        let (results, mut rx) = mpsc::unbounded_channel();

        for index in 0..3 {
            manager
                .submit(request("0", &format!("123{index}"), results.clone(), index))
                .expect("manager is running");
        }

        drop(results);

        // With a single download slot only the first download starts.
        assert_eq!(started_rx.recv().await.as_deref(), Some("1230"));
        assert!(
            started_rx.try_recv().is_err(),
            "a second download started while the first was still running"
        );

        // Releasing the first gate starts the second download, and so on.
        drop(gate_0);
        assert_eq!(started_rx.recv().await.as_deref(), Some("1231"));
        assert!(started_rx.try_recv().is_err());

        drop(gate_1);
        assert_eq!(started_rx.recv().await.as_deref(), Some("1232"));

        drop(gate_2);

        // The requests finish in submission order.
        let mut order = Vec::new();

        for _ in 0..3 {
            let (index, result) = rx.recv().await.expect("the downloads did not finish");
            assert!(matches!(result, Err(Error::Download(_))), "{result:?}");
            order.push(index);
        }

        assert_eq!(order, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn test_rejected_queued_request_does_not_stall_the_queue() {
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let (gate_0, gate_rx_0) = oneshot::channel();
        let (gate_2, gate_rx_2) = oneshot::channel();

        // Only the two valid requests reach the downloader; the rejected one never gets a gate.
        let downloader = Arc::new(GatedDownloader {
            started,
            gates: Mutex::new(VecDeque::from([gate_rx_0, gate_rx_2])),
        });
        let (_download_dir, manager) = start_manager(downloader, S3::for_test(), 1);

        let (results, mut rx) = mpsc::unbounded_channel();
        manager
            .submit(request("0", "first", results.clone(), 0))
            .expect("manager is running");

        // The first download holds the only slot, so the next two requests are queued.
        assert_eq!(started_rx.recv().await.as_deref(), Some("first"));

        manager
            .submit(request("0", "../evil", results.clone(), 1))
            .expect("manager is running");
        manager
            .submit(request("0", "third", results, 2))
            .expect("manager is running");

        drop(gate_0);

        // The queued request with the unsafe id is rejected as soon as it is popped; the request
        // behind it must still start.
        assert_eq!(started_rx.recv().await.as_deref(), Some("third"));

        drop(gate_2);

        // The slot is released before the upload, so the queue drains ahead of the finished
        // request's callback; match outcomes by request index rather than arrival order.
        let mut outcomes = Vec::new();

        for _ in 0..3 {
            outcomes.push(rx.recv().await.expect("every request finishes"));
        }

        outcomes.sort_by_key(|(index, _)| *index);

        assert!(matches!(outcomes[0].1, Err(Error::Download(_))));
        assert!(matches!(outcomes[1].1, Err(Error::InvalidId(_))));
        assert!(matches!(outcomes[2].1, Err(Error::Download(_))));
    }

    #[test]
    fn test_cleanup_stale_downloads() {
        let base = tempfile::tempdir().unwrap();

        let stale = base.path().join(format!("{TEMP_DIR_PREFIX}1234"));
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join("video.mp4"), b"junk").unwrap();

        let unrelated = base.path().join("zeta-unrelated");
        std::fs::create_dir(&unrelated).unwrap();

        assert_eq!(
            crate::tempdir::cleanup_stale_downloads(base.path(), None),
            1
        );
        assert!(!stale.exists());
        assert!(unrelated.exists());
    }
}
