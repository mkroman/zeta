//! Download manager for `yt-dlp`.
//!
//! A long-running task that owns the full lifecycle of a mirror: download requests are submitted
//! through an unbounded mpsc channel, every accepted request runs as its own `yt-dlp` task that
//! streams progress status back to the manager over a second mpsc channel, and completed downloads
//! are uploaded to S3 inline before the requester is notified with the public link.
//!
//! At most [`MAX_CONCURRENT_DOWNLOADS`] downloads run at a time; further requests are queued and
//! started in order as running downloads finish.

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
};

use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use super::{
    s3::S3,
    ytdlp::{self, DownloadedFile, Progress, YtDlp},
};

/// The filename prefix for our temporary download directories.
const TEMP_DIR_PREFIX: &str = "zeta-tiktok-";

/// The default maximum number of downloads that run concurrently.
const MAX_CONCURRENT_DOWNLOADS: usize = 2;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("could not create temporary directory: {0}")]
    TempDir(#[from] std::io::Error),
    #[error("download error: {0}")]
    Download(#[from] ytdlp::Error),
    #[error("upload error: {0}")]
    Upload(#[from] super::s3::Error),
}

/// A request to download the video at `url` and mirror it to the bucket.
pub struct DownloadRequest {
    /// The URL to download.
    pub url: String,
    /// The id of the video being downloaded.
    pub video_id: String,
    /// Called exactly once when the download finishes, successfully or not. On success it
    /// receives the public URL of the mirrored video.
    pub on_finish: OnFinish,
}

impl std::fmt::Debug for DownloadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadRequest")
            .field("url", &self.url)
            .field("video_id", &self.video_id)
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
    /// Temporary download directories left behind by a previous run are removed.
    ///
    /// # Panics
    ///
    /// Panics if called outside of a tokio runtime.
    #[must_use]
    pub fn start(ytdlp: YtDlp, s3: S3) -> Self {
        Self::start_with(ytdlp, s3, MAX_CONCURRENT_DOWNLOADS)
    }

    /// Starts the download manager task with the given maximum number of concurrent downloads.
    fn start_with(ytdlp: YtDlp, s3: S3, max_concurrent: usize) -> Self {
        let removed = cleanup_stale_downloads(&std::env::temp_dir());
        if removed > 0 {
            debug!(removed, "cleaned up stale download directories");
        }

        let (requests, request_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = mpsc::unbounded_channel();

        let manager = Manager {
            ytdlp,
            s3,
            max_concurrent,
            requests: request_rx,
            status_tx,
            status: status_rx,
            active: HashMap::new(),
            queue: VecDeque::new(),
            running: 0,
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
    pub fn submit(&self, request: DownloadRequest) -> Result<(), DownloadRequest> {
        self.requests.send(request).map_err(|err| err.0)
    }
}

/// The state of the download manager task.
struct Manager {
    /// The `yt-dlp` runner used for downloading videos.
    ytdlp: YtDlp,
    /// The S3 client used for uploading videos.
    s3: S3,
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
    /// The number of currently running downloads.
    running: usize,
    /// The id to assign to the next download.
    next_id: u64,
}

/// A running download and the state the manager keeps around it.
struct ActiveDownload {
    /// The request the download was started for.
    request: DownloadRequest,
    /// The temporary directory the video is downloaded into, removed when the download finishes.
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
                    let Some(status) = status else { break };
                    self.on_status(status).await;
                }
            }
        }

        debug!("download manager stopped");
    }

    /// Handles an incoming download request, starting it or queueing it for later.
    fn on_request(&mut self, request: DownloadRequest) {
        if self.running < self.max_concurrent {
            self.start_download(request);
        } else {
            debug!(
                video_id = %request.video_id,
                running = self.running,
                "queued download request"
            );

            self.queue.push_back(request);
        }
    }

    /// Starts a download task for the given request, creating its temporary download directory.
    fn start_download(&mut self, request: DownloadRequest) {
        let id = self.next_id;
        self.next_id += 1;
        self.running += 1;

        let tempdir = match tempfile::Builder::new().prefix(TEMP_DIR_PREFIX).tempdir() {
            Ok(tempdir) => tempdir,
            Err(err) => {
                error!(
                    video_id = %request.video_id,
                    error = %err,
                    "could not create a temporary download directory"
                );

                self.running -= 1;
                (request.on_finish)(Err(Error::TempDir(err)));

                return;
            }
        };

        debug!(
            video_id = %request.video_id,
            %id,
            path = %tempdir.path().display(),
            "starting download"
        );

        let ytdlp = self.ytdlp.clone();
        let task = DownloadTask::new(id, self.status_tx.clone());
        let url = request.url.clone();
        let path = tempdir.path().to_path_buf();

        tokio::spawn(async move {
            let result = ytdlp
                .download_with_progress(&url, &path, |progress| task.progress(progress))
                .await;

            match result {
                Ok(files) => task.completed(files),
                Err(error) => task.failed(error),
            }
        });

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
        match status {
            DownloadStatus::Progress { id, progress } => {
                if let Some(active) = self.active.get_mut(&id) {
                    active.last_progress = Some(progress);

                    debug!(
                        video_id = %active.request.video_id,
                        percent = ?progress.fraction().map(|fraction| format!("{:.0}%", fraction * 100.0)),
                        ?progress,
                        "download progress"
                    );
                }
            }
            DownloadStatus::Completed { id, files } => {
                let Some(active) = self.active.remove(&id) else {
                    return;
                };

                self.finish(active, Ok(files)).await;
            }
            DownloadStatus::Failed { id, error } => {
                let Some(active) = self.active.remove(&id) else {
                    return;
                };

                error!(
                    video_id = %active.request.video_id,
                    error = %error,
                    "could not download video"
                );

                self.finish(active, Err(error)).await;
            }
        }
    }

    /// Finishes a download: uploads the downloaded files to the bucket (if any), notifies the
    /// requester of the result and removes the temporary download directory. Starts the next
    /// queued download, if any.
    async fn finish(
        &mut self,
        active: ActiveDownload,
        result: Result<Vec<DownloadedFile>, ytdlp::Error>,
    ) {
        self.running -= 1;

        let ActiveDownload {
            request: DownloadRequest {
                video_id, on_finish, ..
            },
            tempdir,
            ..
        } = active;

        let result = match result {
            Ok(files) => match Self::upload(&self.s3, &video_id, &files).await {
                Ok(link) => Ok(link),
                Err(err) => {
                    error!(%video_id, error = %err, "could not upload the downloaded video");
                    Err(Error::Upload(err))
                }
            },
            Err(err) => Err(Error::Download(err)),
        };

        // The temporary directory is removed here, after the upload of its contents.
        drop(tempdir);

        on_finish(result);

        self.start_next();
    }

    /// Uploads the downloaded files to the bucket and returns the public URL of the mirrored
    /// video.
    ///
    /// Files that already exist in the bucket under their exact key are not uploaded again.
    ///
    /// # Errors
    ///
    /// Returns an error if the presence of a file could not be checked or a file could not be
    /// uploaded.
    async fn upload(
        s3: &S3,
        video_id: &str,
        files: &[DownloadedFile],
    ) -> Result<String, super::s3::Error> {
        for file in files {
            if file.is_unsupported_codec() {
                warn!(
                    video_id,
                    vcodec = ?file.vcodec,
                    "the video is in a format that isn't supported by browsers"
                );
            }

            let Some(filename) = file.filename() else {
                continue;
            };

            let key = s3.key_for(&filename);

            if s3.object_exists(&key).await? {
                debug!(%key, "skipping upload, object already exists");
                continue;
            }

            s3.upload_file(&file.filepath, &key).await?;
            debug!(%key, "uploaded video to s3");
        }

        Ok(s3.public_url(video_id))
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
        self.send(DownloadStatus::Completed {
            id: self.id,
            files,
        });
        self.status_tx = None;
    }

    /// Reports the download as failed, disarming the guard.
    fn failed(mut self, error: ytdlp::Error) {
        self.send(DownloadStatus::Failed {
            id: self.id,
            error,
        });
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

/// Removes any temporary download directories left behind by a previous run.
///
/// Returns the number of directories removed. Only directories are touched; anything else with a
/// matching name is left alone.
fn cleanup_stale_downloads(base: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(base) else {
        return 0;
    };

    let mut removed = 0;

    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(TEMP_DIR_PREFIX)
            || !entry.file_type().is_ok_and(|file_type| file_type.is_dir())
        {
            continue;
        }

        match std::fs::remove_dir_all(entry.path()) {
            Ok(()) => removed += 1,
            Err(err) => warn!(
                path = %entry.path().display(),
                error = %err,
                "could not remove stale download directory"
            ),
        }
    }

    removed
}

/// Returns the temporary download directories that currently exist.
#[cfg(test)]
fn stale_download_dirs() -> Vec<std::path::PathBuf> {
    std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry.file_name().to_string_lossy().starts_with(TEMP_DIR_PREFIX)
                && entry.file_type().is_ok_and(|file_type| file_type.is_dir())
        })
        .map(|entry| entry.path())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that run a download manager: every manager start cleans up stale
    /// `zeta-tiktok-*` directories in the shared temporary directory, which would otherwise
    /// destroy the active downloads of a concurrently running test.
    static DOWNLOAD_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Writes an executable script that sleeps for the number of seconds given as its last
    /// argument (the download URL), and then exits with a failure.
    fn write_sleeping_script(name: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = std::env::temp_dir().join(format!("zeta-test-{name}-{}.sh", std::process::id()));
        std::fs::write(&script, "#!/bin/sh\nfor last; do :; done\nsleep \"$last\"\nexit 1\n")
            .unwrap();

        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();

        script
    }

    /// Writes an executable script that acts like a successful `yt-dlp` run: it writes a file
    /// into the directory passed via `--paths` and dumps its json.
    fn write_successful_script(name: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = std::env::temp_dir().join(format!("zeta-test-{name}-{}.sh", std::process::id()));
        std::fs::write(
            &script,
            r#"#!/bin/sh
while [ $# -gt 0 ]; do
  if [ "$1" = "--paths" ] && [ -n "$2" ]; then
    dir="$2"
  fi
  shift
done
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

    /// Returns a request that reports its result on the given channel.
    fn request(
        url: &str,
        video_id: &str,
        results: mpsc::UnboundedSender<(usize, Result<String, Error>)>,
        index: usize,
    ) -> DownloadRequest {
        DownloadRequest {
            url: url.to_string(),
            video_id: video_id.to_string(),
            on_finish: Box::new(move |result| {
                let _ = results.send((index, result));
            }),
        }
    }

    #[tokio::test]
    async fn test_failed_download_reports_and_cleans_up() {
        let _guard = DOWNLOAD_TESTS.lock().await;

        let script = write_sleeping_script("manager-sleep-a");
        let manager = DownloadManager::start(
            YtDlp::with_command(script.to_str().unwrap()),
            S3::for_test(),
        );

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
            stale_download_dirs().is_empty(),
            "temporary download directories were not removed: {:?}",
            stale_download_dirs()
        );

        std::fs::remove_file(&script).unwrap();
    }

    #[tokio::test]
    async fn test_completed_download_is_uploaded_and_reported() {
        let _guard = DOWNLOAD_TESTS.lock().await;

        let script = write_successful_script("manager-success");
        let manager = DownloadManager::start(
            YtDlp::with_command(script.to_str().unwrap()),
            S3::for_test(),
        );

        let (results, mut rx) = mpsc::unbounded_channel();
        manager
            .submit(request("https://www.tiktok.com/@user/video/123", "123", results, 0))
            .expect("manager is running");

        // The download succeeds, but the upload fails against the unreachable test endpoint.
        let (_, result) = rx.recv().await.expect("the download did not finish");
        let error = result.expect_err("the upload should have failed");

        assert!(matches!(error, Error::Upload(_)));

        // The temporary download directory was removed.
        assert!(stale_download_dirs().is_empty());

        std::fs::remove_file(&script).unwrap();
    }

    #[tokio::test]
    async fn test_requests_run_serially_when_capped() {
        let _guard = DOWNLOAD_TESTS.lock().await;

        let script = write_sleeping_script("manager-sleep-b");
        let manager = DownloadManager::start_with(
            YtDlp::with_command(script.to_str().unwrap()),
            S3::for_test(),
            1,
        );

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

        assert_eq!(cleanup_stale_downloads(base.path()), 1);
        assert!(!stale.exists());
        assert!(unrelated.exists());
    }
}
