//! Download manager for `yt-dlp`.
//!
//! A long-running task that owns the full lifecycle of a mirror: download requests are submitted
//! through an unbounded mpsc channel, every accepted request runs as its own `yt-dlp` task that
//! streams progress status back to the manager over a second mpsc channel, and completed downloads
//! are uploaded to S3 inline before the requester is notified with the public link.
//!
//! At most the configured number of downloads run at a time; further requests are queued and
//! started in order as running downloads finish.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
};

#[cfg(test)]
use std::path::Path;

use tokio::sync::mpsc;
use tracing::{Instrument, debug, error, warn};
use url::Url;

use super::{
    object_key, public_url_for,
    s3::S3,
    tempdir_builder,
    ytdlp::{self, DownloadedFile, Progress, YtDlp},
};
use crate::url::is_identifier;

#[cfg(test)]
use super::TEMP_DIR_PREFIX;

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

/// A status update sent from a download task to the manager.
#[derive(Debug)]
enum DownloadStatus {
    /// The download made progress.
    Progress { id: u64, progress: Progress },
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
    /// # Panics
    ///
    /// Panics if called outside of a tokio runtime.
    #[must_use]
    pub fn start(
        ytdlp: YtDlp,
        s3: S3,
        download_dir: PathBuf,
        base_dir: Option<tempfile::TempDir>,
        max_concurrent: usize,
    ) -> Self {
        let removed = super::cleanup_stale_downloads(&download_dir, None);
        if removed > 0 {
            debug!(removed, "cleaned up stale download directories");
        }

        let (requests, request_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = mpsc::unbounded_channel();

        let manager = Manager {
            ytdlp,
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

        tokio::spawn(
            manager
                .run()
                .instrument(tracing::info_span!("download_manager")),
        );

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
    /// The `yt-dlp` runner used for downloading media.
    ytdlp: YtDlp,
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
    /// The most recent progress update of the download.
    last_progress: Option<Progress>,
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
                media_id = %request.id,
                running = self.active.len(),
                "queued download request"
            );

            self.queue.push_back(request);
        }
    }

    /// Starts a download task for the given request, creating its temporary download directory.
    fn start_download(&mut self, request: DownloadRequest) {
        if !is_identifier(&request.id, "_-") {
            error!(id = %request.id, "rejecting download with an unsafe id");

            (request.on_finish)(Err(Error::InvalidId(request.id)));

            return;
        }

        let id = self.next_id;
        self.next_id += 1;

        let tempdir = match tempdir_builder().tempdir_in(&self.download_dir) {
            Ok(tempdir) => tempdir,
            Err(err) => {
                error!(
                    media_id = %request.id,
                    error = %err,
                    "could not create a temporary download directory"
                );

                (request.on_finish)(Err(Error::TempDir(err)));

                return;
            }
        };

        debug!(
            media_id = %request.id,
            %id,
            path = %tempdir.path().display(),
            "starting download"
        );

        let ytdlp = self.ytdlp.clone();
        let task = DownloadTask::new(id, self.status_tx.clone());
        let url = request.url.clone();
        let media_id = request.id.clone();
        let path = tempdir.path().to_path_buf();

        // The span carries the media id: the download runs in its own task, so without it the
        // yt-dlp and upload diagnostics below only ever reach stdout.
        let span = tracing::info_span!("download_media", media_id = %media_id);

        tokio::spawn(
            async move {
                let result = ytdlp
                    .download_with_progress(&url, &media_id, &path, |progress| {
                        task.progress(progress);
                    })
                    .await;

                match result {
                    Ok(files) => task.completed(files),
                    Err(error) => task.failed(error),
                }
            }
            .instrument(span),
        );

        self.active.insert(
            id,
            ActiveDownload {
                request,
                tempdir,
                last_progress: None,
            },
        );
    }

    /// Handles a status update reported by a download task.
    async fn on_status(&mut self, status: DownloadStatus) {
        let (id, result) = match status {
            DownloadStatus::Progress { id, progress } => {
                if let Some(active) = self.active.get_mut(&id) {
                    active.last_progress = Some(progress);

                    debug!(
                        media_id = %active.request.id,
                        percent = ?progress.fraction().map(|fraction| format!("{:.0}%", fraction * 100.0)),
                        ?progress,
                        "download progress"
                    );
                }

                return;
            }
            DownloadStatus::Completed { id, files } => (id, Ok(files)),
            DownloadStatus::Failed { id, error } => (id, Err(error)),
        };

        let Some(active) = self.active.remove(&id) else {
            return;
        };

        if let Err(error) = &result {
            error!(
                media_id = %active.request.id,
                error = %error,
                "could not download media"
            );
        }

        self.finish(active, result).await;
    }

    /// Finishes a download: uploads the downloaded files to the bucket (if any), notifies the
    /// requester of the result and removes the temporary download directory. Starts the next
    /// queued download, if any.
    async fn finish(
        &mut self,
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
            Ok(files) => {
                match Self::upload(&self.s3, &prefix, &public_url_base, &id, &files).await {
                    Ok(link) => Ok(link),
                    Err(err) => {
                        error!(media_id = %id, error = %err, "could not upload the downloaded media");
                        Err(Error::Upload(err))
                    }
                }
            }
            Err(err) => Err(Error::Download(err)),
        };

        // The temporary directory and everything in it are removed here, both on success and on
        // failure, so that downloaded files never outlive the download. The path is captured
        // before `close`, since it is no longer valid afterwards.
        let tempdir_path = tempdir.path().to_path_buf();

        if let Err(err) = tempdir.close() {
            error!(path = %tempdir_path.display(), error = %err, "could not remove the temporary download directory");
        }

        on_finish(result);

        self.start_next();
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
                    media_id = id,
                    vcodec = ?file.vcodec,
                    "the media is in a format that isn't supported by browsers"
                );
            }

            let Some(filename) = file.filename() else {
                continue;
            };

            let key = object_key(prefix, &filename);

            if s3.object_exists(&key).await? {
                debug!(%key, "skipping upload, object already exists");
                continue;
            }

            s3.upload_file(&file.filepath, &key).await?;
            debug!(%key, "uploaded media to s3");
        }

        Ok(public_url_for(public_url_base, id))
    }

    /// Starts the next queued download, if there is one.
    fn start_next(&mut self) {
        if let Some(request) = self.queue.pop_front() {
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

    /// Reports a progress update of the download.
    fn progress(&self, progress: Progress) {
        self.send(DownloadStatus::Progress {
            id: self.id,
            progress,
        });
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

/// Returns the temporary download directories that currently exist under `base`.
#[cfg(test)]
fn stale_download_dirs(base: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(base)
        .unwrap()
        .flatten()
        .filter(super::is_temp_download_dir)
        .map(|entry| entry.path())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `yt-dlp` stand-in that fails after sleeping for the download URL.
    const SLEEPING_SCRIPT_BODY: &str = "#!/bin/sh\nfor last; do :; done\nsleep \"$last\"\nexit 1\n";

    /// A `yt-dlp` stand-in that writes a file into `--paths` and dumps its json.
    const SUCCESSFUL_SCRIPT_BODY: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
  if [ "$1" = "--paths" ] && [ -n "$2" ]; then
    dir="$2"
  fi
  shift
done
printf junk > "$dir/123.mp4"
printf '{"id": "123", "requested_downloads": [{"filepath": "%s/123.mp4", "id": "123", "ext": "mp4", "vcodec": "avc1.640029", "acodec": "mp4a.40.2"}]}' "$dir"
"#;

    /// Writes an executable script that sleeps for the number of seconds given as its last
    /// argument (the download URL), and then exits with a failure.
    fn write_sleeping_script(name: &str) -> PathBuf {
        crate::mirror::write_test_script(name, SLEEPING_SCRIPT_BODY)
    }

    /// Writes an executable script that acts like a successful `yt-dlp` run: it writes a file
    /// into the directory passed via `--paths` and dumps its json.
    fn write_successful_script(name: &str) -> PathBuf {
        crate::mirror::write_test_script(name, SUCCESSFUL_SCRIPT_BODY)
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
        }
    }

    /// Starts a manager with its own isolated download directory, so tests cannot interfere with
    /// each other.
    fn start_manager(script: &Path, max_concurrent: usize) -> (tempfile::TempDir, DownloadManager) {
        let download_dir = tempfile::tempdir().unwrap();
        let manager = DownloadManager::start(
            YtDlp::with_command(script.to_str().unwrap()),
            S3::for_test(),
            download_dir.path().to_path_buf(),
            None,
            max_concurrent,
        );

        (download_dir, manager)
    }

    #[tokio::test]
    async fn test_invalid_id_is_rejected() {
        let script = write_sleeping_script("manager-invalid-id");
        let (_download_dir, manager) = start_manager(&script, 2);

        let (results, mut rx) = mpsc::unbounded_channel();
        manager
            .submit(request("0", "../evil", results, 0))
            .expect("manager is running");

        let (_, result) = rx.recv().await.expect("the download did not finish");
        let error = result.expect_err("the request should have been rejected");

        assert!(matches!(error, Error::InvalidId(_)));

        std::fs::remove_file(&script).unwrap();
    }

    #[tokio::test]
    async fn test_failed_download_reports_and_cleans_up() {
        let script = write_sleeping_script("manager-sleep-a");
        let (download_dir, manager) = start_manager(&script, 2);

        let (results, mut rx) = mpsc::unbounded_channel();
        manager
            .submit(request("0", "123", results, 0))
            .expect("manager is running");

        // The script sleeps for zero seconds, emits no output and exits with a failure.
        let (_, result) = rx.recv().await.expect("the download did not finish");
        let error = result.expect_err("the download should have failed");

        assert!(matches!(error, Error::Download(_)));

        // The temporary download directory was removed.
        assert!(
            stale_download_dirs(download_dir.path()).is_empty(),
            "temporary download directories were not removed: {:?}",
            stale_download_dirs(download_dir.path())
        );

        std::fs::remove_file(&script).unwrap();
    }

    #[tokio::test]
    async fn test_completed_download_is_uploaded_and_reported() {
        let script = write_successful_script("manager-success");
        let (download_dir, manager) = start_manager(&script, 2);

        let (results, mut rx) = mpsc::unbounded_channel();
        manager
            .submit(request(
                "https://www.tiktok.com/@user/video/123",
                "123",
                results,
                0,
            ))
            .expect("manager is running");

        // The download succeeds, but the upload fails against the unreachable test endpoint.
        let (_, result) = rx.recv().await.expect("the download did not finish");
        let error = result.expect_err("the upload should have failed");

        assert!(matches!(error, Error::Upload(_)));

        // The temporary download directory was removed.
        assert_eq!(
            stale_download_dirs(download_dir.path()),
            Vec::<PathBuf>::new()
        );

        std::fs::remove_file(&script).unwrap();
    }

    #[tokio::test]
    async fn test_requests_run_serially_when_capped() {
        let script = write_sleeping_script("manager-sleep-b");
        let (_download_dir, manager) = start_manager(&script, 1);

        let (results, mut rx) = mpsc::unbounded_channel();

        // The first request sleeps for two seconds; the queued ones finish without sleeping. With
        // a single download slot they must still finish in submission order.
        for (index, url) in ["2", "0", "0"].iter().enumerate() {
            manager
                .submit(request(url, &format!("123{index}"), results.clone(), index))
                .expect("manager is running");
        }

        drop(results);

        let mut order = Vec::new();

        for _ in 0..3 {
            let (index, _) = rx.recv().await.expect("the downloads did not finish");
            order.push(index);
        }

        assert_eq!(order, vec![0, 1, 2]);

        std::fs::remove_file(&script).unwrap();
    }

    #[test]
    fn test_cleanup_stale_downloads() {
        let base = tempfile::tempdir().unwrap();

        let stale = base.path().join(format!("{TEMP_DIR_PREFIX}1234"));
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join("video.mp4"), b"junk").unwrap();

        let unrelated = base.path().join("zeta-unrelated");
        std::fs::create_dir(&unrelated).unwrap();

        assert_eq!(crate::mirror::cleanup_stale_downloads(base.path(), None), 1);
        assert!(!stale.exists());
        assert!(unrelated.exists());
    }
}
