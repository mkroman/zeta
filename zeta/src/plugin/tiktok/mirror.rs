//! Mirroring of TikTok videos to S3.
//!
//! The actual mirroring is managed by [`DownloadManager`], a long-running task that downloads
//! videos with `yt-dlp` (streaming progress status back), uploads them to the bucket and notifies
//! the requester with the public link. Downloads happen inside temporary directories that are
//! owned by the manager and removed when the download finishes; directories left behind by a
//! killed or crashed process are removed on startup.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use tracing::{debug, error, warn};

use super::{
    manager::{DownloadManager, DownloadRequest},
    s3::S3,
    ytdlp::YtDlp,
};

/// Mirrors TikTok videos: downloads them with `yt-dlp` and uploads them to S3.
#[derive(Clone)]
pub struct Mirror {
    /// The S3 client used for checking whether videos are already mirrored.
    s3: S3,
    /// The `yt-dlp` runner used for downloading videos.
    ytdlp: YtDlp,
    /// The set of videos that are currently being mirrored.
    in_flight: Arc<Mutex<HashSet<String>>>,
    /// The download manager, started by [`Mirror::start_downloads`].
    manager: Option<DownloadManager>,
}

impl Mirror {
    /// Creates a mirror from the environment configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the S3 configuration is missing or invalid.
    pub fn from_env() -> Result<Self, super::s3::Error> {
        Ok(Self {
            s3: S3::from_env()?,
            ytdlp: YtDlp::from_env(),
            in_flight: Arc::default(),
            manager: None,
        })
    }

    /// Starts the download manager task.
    ///
    /// Must be called before [`Mirror::ensure_mirrored`], when the plugin is loaded.
    pub fn start_downloads(&mut self) {
        if self.manager.is_none() {
            self.manager = Some(DownloadManager::start(self.ytdlp.clone(), self.s3.clone()));
        }
    }

    /// Returns the object key for the given file name.
    fn key_for(&self, filename: &str) -> String {
        self.s3.key_for(filename)
    }

    /// Returns the public, viewable URL for the given video id.
    fn public_url(&self, video_id: &str) -> String {
        self.s3.public_url(video_id)
    }

    /// Returns whether a video with the given id has already been mirrored.
    ///
    /// This is a fast-path check that assumes the common `.mp4` container; a video that was
    /// mirrored under a different container is only caught by the exact-key check performed
    /// by the download manager after a new download.
    async fn is_mirrored(&self, video_id: &str) -> Result<bool, super::s3::Error> {
        let key = self.key_for(&format!("{video_id}.mp4"));

        self.s3.object_exists(&key).await
    }

    /// Returns `false` if the video is already being mirrored, otherwise marks it as in-flight.
    fn mark_in_flight(&self, video_id: &str) -> bool {
        self.in_flight
            .lock()
            .expect("in-flight lock is poisoned")
            .insert(video_id.to_string())
    }

    /// Removes a video from the in-flight set.
    fn clear_in_flight(&self, video_id: &str) {
        self.in_flight
            .lock()
            .expect("in-flight lock is poisoned")
            .remove(video_id);
    }

    /// Mirrors the video at `url` unless it has already been mirrored or a mirror of it is
    /// already in progress.
    ///
    /// If the video has already been mirrored, its public link is returned. Otherwise the download
    /// is submitted to the download manager, which calls `on_mirrored` with the public link once
    /// it has been downloaded and uploaded, and `None` is returned.
    ///
    /// # Errors
    ///
    /// Returns an error if it could not be checked whether the video has already been mirrored.
    pub async fn ensure_mirrored<F>(
        &self,
        url: &str,
        video_id: &str,
        on_mirrored: F,
    ) -> Result<Option<String>, super::s3::Error>
    where
        F: FnOnce(String) + Send + 'static,
    {
        if self.is_mirrored(video_id).await? {
            let link = self.public_url(video_id);
            debug!(%video_id, %link, "video has already been mirrored");

            return Ok(Some(link));
        }

        if !self.mark_in_flight(video_id) {
            debug!(%video_id, "video is already being mirrored");

            return Ok(None);
        }

        let Some(manager) = &self.manager else {
            warn!(%video_id, "download manager is not running");
            self.clear_in_flight(video_id);

            return Ok(None);
        };

        let mirror = self.clone();
        let url = url.to_string();
        let video_id = video_id.to_string();

        let request = DownloadRequest {
            url,
            video_id: video_id.clone(),
            on_finish: Box::new(move |result| {
                // The in-flight mark is always cleared, also on failure, so that a later link
                // triggers a new attempt. Errors are logged by the download manager.
                mirror.clear_in_flight(&video_id);

                if let Ok(link) = result {
                    on_mirrored(link);
                }
            }),
        };

        if let Err(request) = manager.submit(request) {
            error!(
                video_id = %request.video_id,
                "could not submit the download, the manager is not running"
            );

            self.clear_in_flight(&request.video_id);
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// Returns a mirror that never talks to S3 and has no manager running.
    fn mirror_for_test() -> Mirror {
        Mirror {
            s3: S3::for_test(),
            ytdlp: YtDlp::with_command("yt-dlp"),
            in_flight: Arc::default(),
            manager: None,
        }
    }

    #[test]
    fn test_in_flight_tracking() {
        let mirror = mirror_for_test();

        assert!(mirror.mark_in_flight("123"));
        // A video that is already being mirrored is not marked again.
        assert!(!mirror.mark_in_flight("123"));
        assert!(mirror.mark_in_flight("456"));

        mirror.clear_in_flight("123");
        assert!(mirror.mark_in_flight("123"));
    }

    /// Runs a minimal S3 server that answers every request with a 404, so that the fast-path
    /// check finds no mirror. Returns the mirror pointed at the server and a handle for the
    /// server thread.
    fn mirror_with_missing_objects() -> (Mirror, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            let mut buffer = [0; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buffer);

            stream
                .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                .unwrap();
        });

        let mirror = Mirror {
            s3: S3::with_endpoint(&format!("http://{address}")),
            ytdlp: YtDlp::with_command("yt-dlp"),
            in_flight: Arc::default(),
            manager: None,
        };

        (mirror, server)
    }

    #[tokio::test]
    async fn test_ensure_mirrored_without_manager_clears_in_flight() {
        let (mirror, server) = mirror_with_missing_objects();

        let result = mirror
            .ensure_mirrored("https://www.tiktok.com/@user/video/123", "123", |_| {})
            .await
            .expect("the fast-path check should succeed");

        assert!(result.is_none());
        // The video must not remain marked as in-flight, so a later mirror can be requested.
        assert!(mirror.mark_in_flight("123"));

        server.join().unwrap();
    }
}
