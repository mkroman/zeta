//! Unit tests for the manager's request intake, download and upload lifecycles.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::sync::oneshot;
use zeta_test_support::wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};

use super::*;

use super::upload::UploadCompletion;
use crate::tempdir::TEMP_DIR_PREFIX;
use crate::ytdlp::Progress;

/// Returns the temporary download directories that currently exist under `base`.
fn stale_download_dirs(base: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(base)
        .unwrap()
        .flatten()
        .filter(crate::tempdir::is_temp_download_dir)
        .map(|entry| entry.path())
        .collect()
}

/// Waits until `server` has received at least one request, i.e. an upload reached its
/// endpoint and is stalled there.
async fn wait_for_s3_request(server: &MockServer) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let requests = server
                .received_requests()
                .await
                .expect("the server tracks requests");

            if !requests.is_empty() {
                break;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the upload should reach s3");
}

/// A minimal S3 endpoint whose presence checks block until [`release`](Self::release) is
/// called, then every request answers `404 Not Found`.
///
/// This lets a test stall every upload, prove the backlog bound and then free all of them at
/// once; a fixed wiremock delay cannot release a request that is already waiting.
struct StalledS3 {
    /// The address the endpoint listens on.
    address: std::net::SocketAddr,
    /// Notified once per request received.
    arrived: mpsc::UnboundedReceiver<()>,
    /// Whether stalled presence checks may answer.
    release: tokio::sync::watch::Sender<bool>,
    /// The task accepting requests.
    task: tokio::task::JoinHandle<()>,
}

impl StalledS3 {
    /// Starts the endpoint on a random local port.
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the stalled s3 endpoint");
        let address = listener.local_addr().expect("read the stalled s3 address");
        let (arrived, arrived_rx) = mpsc::unbounded_channel();
        let (release, release_rx) = tokio::sync::watch::channel(false);

        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let arrived = arrived.clone();
                let mut release_rx = release_rx.clone();

                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};

                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 1024];

                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        let Ok(read) = stream.read(&mut buffer).await else {
                            return;
                        };

                        if read == 0 {
                            return;
                        }

                        request.extend_from_slice(&buffer[..read]);
                    }

                    let _ = arrived.send(());

                    if request.starts_with(b"HEAD ") {
                        let _ = release_rx.wait_for(|released| *released).await;
                    }

                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        Self {
            address,
            arrived: arrived_rx,
            release,
            task,
        }
    }

    /// Waits until `count` requests reached the endpoint.
    async fn wait_for_requests(&mut self, count: usize) {
        for _ in 0..count {
            tokio::time::timeout(Duration::from_secs(5), self.arrived.recv())
                .await
                .expect("the upload should reach s3")
                .expect("the endpoint keeps receiving");
        }
    }

    /// Lets every stalled presence check answer `404 Not Found`.
    fn release(&self) {
        self.release.send(true).expect("the endpoint is listening");
    }
}

impl Drop for StalledS3 {
    fn drop(&mut self) {
        self.task.abort();
    }
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

/// A downloader fake that completes every download while reporting each start, so a test can
/// tell an admitted download from one held behind the upload backlog.
struct SignallingCompletingDownloader {
    /// The media ids of the downloads as they start, in order.
    started: mpsc::UnboundedSender<String>,
}

impl Downloader for SignallingCompletingDownloader {
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

        let filepath = output_dir.join(format!("{id}.mp4"));
        std::fs::write(&filepath, b"junk").expect("write the fake download");

        let files = vec![DownloadedFile {
            filepath,
            vcodec: None,
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
async fn test_zero_concurrency_is_clamped_to_one() {
    let (_download_dir, manager) =
        start_manager(Arc::new(RecordingDownloader::default()), S3::for_test(), 0);

    let (results, mut rx) = mpsc::unbounded_channel();
    manager
        .submit(request("0", "123", results, 0))
        .expect("manager is running");

    // A zero concurrency would leave the request queued forever; the clamp makes it run.
    let (index, result) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("a zero-concurrency manager should still start the download")
        .expect("the result channel stays open");

    assert_eq!(index, 0);
    assert!(matches!(result, Err(Error::Download(_))), "{result:?}");
}

#[tokio::test]
async fn test_full_intake_channel_sheds_on_the_submitter() {
    let (_download_dir, manager) =
        start_manager(Arc::new(RecordingDownloader::default()), S3::for_test(), 1);

    let (results, mut rx) = mpsc::unbounded_channel();

    // A current-thread runtime does not poll the manager task while this loop runs, so the
    // channel is still full when the request past its capacity is submitted.
    for index in 0..=REQUEST_CHANNEL_CAPACITY {
        manager
            .submit(request("0", &format!("{index}"), results.clone(), index))
            .expect("a full channel is shed by submit, not returned");
    }

    let (index, result) = rx
        .try_recv()
        .expect("the request past the channel capacity is shed immediately");

    assert_eq!(index, REQUEST_CHANNEL_CAPACITY);
    assert!(matches!(result, Err(Error::Busy)), "{result:?}");
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
    // The presence check of the first download's upload never answers within the test, so
    // only releasing the slot before the upload lets the second download start. The server
    // is exclusive: the test ends with the request in flight, and a pooled server would be
    // borrowed by another test, which then records this request as one of its own.
    let server = MockServer::builder().start().await;
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

    // The second download's failure is handled while the first upload is still stalled, so
    // only its callback arrives; the first request still waits for its upload.
    let (index, result) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the second download should be reported while the first is uploading")
        .expect("the second request should finish");
    assert_eq!(index, 1);
    assert!(matches!(result, Err(Error::Download(_))));

    assert!(
        rx.try_recv().is_err(),
        "the first request was reported before its upload finished"
    );
}

#[tokio::test]
async fn test_stalled_upload_does_not_block_request_intake() {
    // The first download's upload stalls on its presence check. A request submitted once it
    // is stuck there must still be admitted and start, and its status must still be handled.
    // The server is exclusive: the test ends with the request in flight, and a pooled server
    // would be borrowed by another test, which then records this request as its own.
    let server = MockServer::builder().start().await;
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

    assert_eq!(started_rx.recv().await.as_deref(), Some("first"));
    wait_for_s3_request(&server).await;

    manager
        .submit(request("0", "second", results, 1))
        .expect("manager is running");

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .expect("a request submitted during a stalled upload should start")
            .as_deref(),
        Some("second")
    );

    drop(gate);

    let (index, result) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the second download's status should be handled during the upload")
        .expect("the second request should finish");
    assert_eq!(index, 1);
    assert!(matches!(result, Err(Error::Download(_))));

    assert!(
        rx.try_recv().is_err(),
        "the first request was reported before its upload finished"
    );
}

#[tokio::test]
async fn test_upload_backlog_is_bounded_and_frees_the_queue() {
    // Every upload stalls on its presence check, so completed downloads accumulate until the
    // pipeline is full and backpressure stops the download queue.
    let mut server = StalledS3::start().await;
    let s3 = S3::with_endpoint(&format!("http://{}", server.address));
    let (started, mut started_rx) = mpsc::unbounded_channel();
    let (_download_dir, manager) =
        start_manager(Arc::new(SignallingCompletingDownloader { started }), s3, 1);

    let (results, mut rx) = mpsc::unbounded_channel();
    let capacity = UPLOAD_CONCURRENCY + UPLOAD_BACKLOG;

    for index in 0..capacity {
        manager
            .submit(request(
                "0",
                &format!("fill{index}"),
                results.clone(),
                index,
            ))
            .expect("manager is running");
    }

    // Every fill fits: with a single download slot they still all start because the upload
    // pipeline holds `UPLOAD_CONCURRENCY` running uploads and `UPLOAD_BACKLOG` waiting ones.
    for index in 0..capacity {
        let expected = format!("fill{index}");
        let started_id = tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .expect("every fill should be admitted")
            .expect("the started channel stays open");

        assert_eq!(started_id, expected);
    }

    // Both upload slots are now stalled on their presence check, so the backlog below is
    // held by the upload pipeline rather than by connections still being opened.
    server.wait_for_requests(UPLOAD_CONCURRENCY).await;

    // The last fill completes shortly after it starts and fills the backlog, so a request
    // submitted now is accepted but must not start while the uploads are stalled.
    manager
        .submit(request("0", "overflow", results, capacity))
        .expect("manager is running");

    assert!(
        tokio::time::timeout(Duration::from_millis(200), started_rx.recv())
            .await
            .is_err(),
        "a download started while the upload backlog was full"
    );

    // Free the stalled uploads: the backlog drains, the queued request starts and every
    // request is reported exactly once.
    server.release();

    let started_id = tokio::time::timeout(Duration::from_secs(10), started_rx.recv())
        .await
        .expect("the queued request should start once the upload backlog drains")
        .expect("the started channel stays open");
    assert_eq!(started_id, "overflow");

    let mut callbacks = Vec::new();

    for _ in 0..=capacity {
        let (index, result) = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("every request should be reported")
            .expect("the result channel stays open");

        assert!(matches!(result, Err(Error::Upload(_))), "{result:?}");
        callbacks.push(index);
    }

    callbacks.sort_unstable();
    assert_eq!(callbacks, (0..=capacity).collect::<Vec<_>>());
}

#[tokio::test]
async fn test_full_queue_rejects_with_busy_and_preserves_admission_order() {
    let (started, mut started_rx) = mpsc::unbounded_channel();
    let (gate_0, gate_rx_0) = oneshot::channel();
    let (_gate_1, gate_rx_1) = oneshot::channel();

    // Only the active download and the first queued download ever reach the downloader; the
    // rest stay queued until this test ends.
    let downloader = Arc::new(GatedDownloader {
        started,
        gates: Mutex::new(VecDeque::from([gate_rx_0, gate_rx_1])),
    });
    let (_download_dir, manager) = start_manager(downloader, S3::for_test(), 1);

    let (results, mut rx) = mpsc::unbounded_channel();
    manager
        .submit(request("0", "first", results.clone(), 0))
        .expect("manager is running");

    assert_eq!(started_rx.recv().await.as_deref(), Some("first"));

    // The active download holds the only slot, so these fill the accepted-request queue.
    for index in 0..MAX_QUEUED_REQUESTS {
        manager
            .submit(request(
                "0",
                &format!("queued{index}"),
                results.clone(),
                index + 1,
            ))
            .expect("manager is running");
    }

    manager
        .submit(request(
            "0",
            "overflow",
            results.clone(),
            MAX_QUEUED_REQUESTS + 1,
        ))
        .expect("manager is running");

    // The request over the cap is rejected immediately rather than retained.
    let (index, result) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the overflowing request should be rejected while the queue is full")
        .expect("the result channel stays open");

    assert_eq!(index, MAX_QUEUED_REQUESTS + 1);
    assert!(matches!(result, Err(Error::Busy)), "{result:?}");
    assert!(
        rx.try_recv().is_err(),
        "only the overflowing request should have finished"
    );

    // Releasing the active download admits the oldest queued request, not the new one.
    drop(gate_0);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .expect("the queued request should start once the slot is free")
            .as_deref(),
        Some("queued0")
    );
}

#[tokio::test]
async fn test_cancelled_upload_reports_once_and_cleans_up() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path().to_path_buf();
    let (results, mut rx) = mpsc::unbounded_channel();
    let completion = UploadCompletion::new(
        Box::new(move |result| {
            let _ = results.send(result);
        }),
        tempdir,
    );

    // Dropping without completing stands in for a cancelled or panicking upload task.
    drop(completion);

    let result = rx.try_recv().expect("the callback runs on cancellation");
    assert!(matches!(result, Err(Error::UploadCancelled)));
    assert!(rx.try_recv().is_err(), "the callback must run exactly once");
    assert!(
        !path.exists(),
        "the temporary download directory was not removed"
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
