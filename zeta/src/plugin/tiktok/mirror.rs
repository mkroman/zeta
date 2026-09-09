//! Mirroring of TikTok videos to S3.
//!
//! Downloads happen inside temporary directories that are removed on every in-process path,
//! including errors, panics and cancelled tasks. Directories left behind by a killed or crashed
//! process are removed on startup.

use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
};

use tracing::{debug, error, warn};

use super::{s3::S3, ytdlp::YtDlp};

/// The filename prefix for our temporary download directories.
const TEMP_DIR_PREFIX: &str = "zeta-tiktok-";

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("could not create temporary directory: {0}")]
    TempDir(#[from] std::io::Error),
    #[error("download error: {0}")]
    Download(#[from] super::ytdlp::Error),
    #[error("upload error: {0}")]
    Upload(#[from] super::s3::Error),
}

/// Mirrors TikTok videos: downloads them with `yt-dlp` and uploads them to S3.
#[derive(Clone)]
pub struct Mirror {
    /// The S3 client used for uploading videos.
    s3: S3,
    /// The `yt-dlp` runner used for downloading videos.
    ytdlp: YtDlp,
    /// The set of videos that are currently being mirrored.
    in_flight: Arc<Mutex<HashSet<String>>>,
}

impl Mirror {
    /// Creates a mirror from the environment configuration.
    ///
    /// Temporary download directories left behind by a previous run are removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the S3 configuration is missing or invalid.
    pub fn from_env() -> Result<Self, super::s3::Error> {
        let mirror = Self {
            s3: S3::from_env()?,
            ytdlp: YtDlp::from_env(),
            in_flight: Arc::default(),
        };

        let removed = cleanup_stale_downloads(&std::env::temp_dir());
        if removed > 0 {
            debug!(removed, "cleaned up stale download directories");
        }

        Ok(mirror)
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
    /// during [`Mirror::mirror_video`].
    async fn is_mirrored(&self, video_id: &str) -> Result<bool, Error> {
        let key = self.key_for(&format!("{video_id}.mp4"));

        Ok(self.s3.object_exists(&key).await?)
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
    /// If the video has already been mirrored, its public link is returned. Otherwise the
    /// download and upload happens in a background task that calls `on_mirrored` with the public
    /// link on success, and `None` is returned.
    ///
    /// # Errors
    ///
    /// Returns an error if it could not be checked whether the video has already been mirrored.
    pub async fn ensure_mirrored<F>(
        &self,
        url: &str,
        video_id: &str,
        on_mirrored: F,
    ) -> Result<Option<String>, Error>
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

        let mirror = self.clone();
        let url = url.to_string();
        let video_id = video_id.to_string();

        tokio::spawn(async move {
            // The guard ensures the video is removed from the in-flight set even if the task
            // panics or is cancelled.
            let _guard = InFlightGuard::new(&mirror, &video_id);

            match mirror.mirror_video(&url, &video_id).await {
                Ok(link) => {
                    debug!(%video_id, %link, "mirrored video");
                    on_mirrored(link);
                }
                Err(err) => error!(%video_id, error = %err, "could not mirror video"),
            }
        });

        Ok(None)
    }

    /// Downloads the video at `url` and uploads it to the bucket.
    ///
    /// Files that already exist in the bucket under their exact key are not uploaded again.
    ///
    /// Returns the public URL of the mirrored video.
    ///
    /// # Errors
    ///
    /// Returns an error if the video could not be downloaded or uploaded.
    async fn mirror_video(&self, url: &str, video_id: &str) -> Result<String, Error> {
        // The directory is removed when dropped, which happens on every path out of this
        // function — including errors, panics and cancelled tasks.
        let tempdir = tempfile::Builder::new().prefix(TEMP_DIR_PREFIX).tempdir()?;
        let downloads = self.ytdlp.download(url, tempdir.path()).await?;

        for download in &downloads {
            if download.is_unsupported_codec() {
                warn!(video_id, vcodec = ?download.vcodec, "the video is in a format that isn't supported by browsers");
            }

            let Some(filename) = download.filename() else {
                continue;
            };

            let key = self.key_for(&filename);

            if self.s3.object_exists(&key).await? {
                debug!(%key, "skipping upload, object already exists");
                continue;
            }

            self.s3.upload_file(&download.filepath, &key).await?;
            debug!(%key, "uploaded video to s3");
        }

        Ok(self.public_url(video_id))
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

/// Removes a video from its mirror's in-flight set when dropped, so that a panicked or cancelled
/// task doesn't leave the video permanently marked as in-flight.
struct InFlightGuard {
    mirror: Mirror,
    video_id: String,
}

impl InFlightGuard {
    fn new(mirror: &Mirror, video_id: &str) -> Self {
        Self {
            mirror: mirror.clone(),
            video_id: video_id.to_string(),
        }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.mirror.clear_in_flight(&self.video_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes an executable script that creates a file inside the directory passed via
    /// `--paths` and then exits with a failure, simulating a crashed download that leaves
    /// files behind.
    fn write_leaking_script() -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = std::env::temp_dir().join(format!("zeta-test-script-{}.sh", std::process::id()));
        std::fs::write(
            &script,
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"--paths\" ] && [ -n \"$2\" ]; then\n    printf leaked > \"$2/leaked.bin\"\n  fi\n  shift\ndone\nexit 1\n",
        )
        .unwrap();

        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();

        script
    }

    /// Returns the temporary download directories that currently exist.
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

    #[tokio::test]
    async fn test_tempdir_removed_on_download_failure() {
        let script = write_leaking_script();
        let mirror = Mirror {
            s3: S3::for_test(),
            ytdlp: YtDlp::with_command(script.to_str().unwrap()),
            in_flight: Arc::default(),
        };

        // The script leaks a file into the output directory before failing; the temporary
        // directory must still be removed along with its contents.
        let result = mirror
            .mirror_video("https://www.tiktok.com/@user/video/123", "123")
            .await;

        assert!(result.is_err());

        let leftovers = stale_download_dirs();
        assert!(
            leftovers.is_empty(),
            "temporary download directories were not removed: {leftovers:?}"
        );

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
