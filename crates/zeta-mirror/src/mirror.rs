//! The shared media mirror service.
//!
//! [`Mirror`] checks the bucket for an existing mirror before every request, keeps the set of
//! media that is currently being mirrored so concurrent requests for the same media do not
//! download it twice, and submits downloads to the download manager. The manager owns the
//! download and upload lifecycle and is started by the host at startup, or lazily on the first
//! request when no explicit startup call was made.

#[cfg(test)]
mod tests;

use std::{
    collections::HashSet,
    fmt,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use parking_lot::Mutex;
use tracing::{debug, error, warn};
use url::Url;

use crate::{
    MediaId, MirrorConfig, MirrorError,
    links::{object_key, public_url_for},
    manager::{DownloadManager, DownloadRequest},
    s3::S3,
    tempdir::{STALE_DOWNLOAD_MAX_AGE, cleanup_stale_downloads, tempdir_builder},
    ytdlp::{YtDlp, YtDlpOptions},
};

/// The media currently being mirrored, keyed by `(prefix, id)`.
///
/// No user code runs under the lock — the critical sections only insert into or remove from the
/// set using trusted keys — so a panic cannot leave it half-updated, which is what makes this
/// non-poisoning lock sound.
type InFlight = Mutex<HashSet<(String, String)>>;

/// Mirrors media: downloads it with `yt-dlp` and uploads it to S3.
///
/// A single instance is shared by all plugins; it is created from [`MirrorConfig`] at startup and
/// published to the context's shared state.
pub struct Mirror {
    /// The S3 client used for checking whether media is already mirrored.
    s3: S3,
    /// The `yt-dlp` runner used for downloading media.
    ytdlp: YtDlp,
    /// The directory that downloads are buffered in, when configured.
    download_dir: Option<PathBuf>,
    /// The maximum number of downloads that run concurrently.
    max_concurrent: usize,
    /// The set of media that is currently being mirrored, by (prefix, id).
    in_flight: Arc<InFlight>,
    /// The download manager, started by the host during startup with [`Mirror::start`]; empty
    /// until then, and `Some(None)` when it could not be built.
    manager: OnceLock<Option<DownloadManager>>,
}

// Manual implementation: the S3 client prints its own redacted view, and the download manager
// is summarized by whether it has started instead of exposing its mailbox.
impl fmt::Debug for Mirror {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mirror")
            .field("s3", &self.s3)
            .field("ytdlp", &self.ytdlp)
            .field("download_dir", &self.download_dir)
            .field("max_concurrent", &self.max_concurrent)
            .field("in_flight", &self.in_flight.lock().len())
            .field(
                "manager_started",
                &self.manager.get().is_some_and(Option::is_some),
            )
            .finish_non_exhaustive()
    }
}

impl Mirror {
    /// Creates a mirror from the shared `[mirror]` configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the S3 configuration is incomplete or invalid.
    pub fn from_config(config: &MirrorConfig) -> Result<Self, MirrorError> {
        let ytdlp = YtDlp::new(YtDlpOptions {
            command: config.ytdlp_command.clone(),
            max_filesize: config.max_filesize.clone(),
            download_timeout: config.download_timeout,
        });
        let s3 = S3::try_new(config)?;

        Ok(Self::new(
            s3,
            ytdlp,
            config.download_dir.clone(),
            config.max_concurrent_downloads,
        ))
    }

    /// Creates a mirror using the given S3 client and `yt-dlp` runner.
    #[must_use]
    pub(crate) fn new(
        s3: S3,
        ytdlp: YtDlp,
        download_dir: Option<PathBuf>,
        max_concurrent: usize,
    ) -> Self {
        Self {
            s3,
            ytdlp,
            download_dir,
            max_concurrent,
            in_flight: Arc::default(),
            manager: OnceLock::new(),
        }
    }

    /// Starts the download manager, if it is not running yet.
    ///
    /// The host calls this during startup, before the IRC connection is established, so the
    /// manager's synchronous setup — sweeping stale download directories and creating the
    /// download directory — does not block a runtime worker on the first mirrored URL.
    ///
    /// Subsequent calls are no-ops. A mirror used without an explicit startup call (as the unit
    /// tests do) still starts its manager lazily on the first mirror request, so this only moves
    /// the work earlier; it is never required for mirroring to work.
    ///
    /// Returns whether the manager is running. This is `false` when the download directory could
    /// not be created, in which case mirroring stays disabled and the failure is cached so later
    /// calls do not retry.
    ///
    /// # Panics
    ///
    /// Panics if called outside of a Tokio runtime, because the download manager spawns its task
    /// on one.
    #[must_use]
    pub fn start(&self) -> bool {
        self.download_manager().is_some()
    }

    /// Builds the download manager, creating its download directory.
    ///
    /// Returns `None` when the directory cannot be created, in which case mirroring stays
    /// disabled.
    fn build_manager(&self) -> Option<DownloadManager> {
        // When no download directory is configured, a random subdirectory of the system
        // temporary directory is used and kept alive for as long as the manager runs; if it
        // cannot be created, mirroring stays disabled rather than writing into the system
        // temporary directory itself. Otherwise the configured directory is created if
        // missing. Either way, temporary directories left behind by a previous run are
        // removed first.
        let dirs = self.download_dir.clone().map_or_else(
            || {
                cleanup_stale_downloads(&std::env::temp_dir(), Some(STALE_DOWNLOAD_MAX_AGE));

                tempdir_builder()
                    .tempdir()
                    .map(|base| {
                        let path = base.path().to_path_buf();

                        (path, Some(base))
                    })
                    .inspect_err(|err| {
                        error!(
                            exception.message = %err,
                            "could not create a download directory, mirroring is disabled"
                        );
                    })
                    .ok()
            },
            |dir| {
                if let Err(err) = std::fs::create_dir_all(&dir) {
                    error!(
                        file.path = %dir.display(),
                        exception.message = %err,
                        "could not create the download directory, mirroring is disabled"
                    );

                    return None;
                }

                Some((dir, None))
            },
        );

        let (download_dir, base_dir) = dirs?;

        Some(DownloadManager::start(
            Arc::new(self.ytdlp.clone()),
            self.s3.clone(),
            download_dir,
            base_dir,
            self.max_concurrent,
        ))
    }

    /// Returns whether media with the given id has already been mirrored under `prefix`.
    ///
    /// This is a fast-path check that assumes the common `.mp4` container; media that was
    /// mirrored under a different container is only caught by the exact-key check performed by
    /// the download manager after a new download.
    async fn is_mirrored(&self, prefix: &str, id: &str) -> Result<bool, MirrorError> {
        let key = object_key(prefix, &format!("{id}.mp4"));

        self.s3.object_exists(&key).await
    }

    /// Marks the media as in-flight, returning the key that clears the mark.
    ///
    /// The returned key is the copy this call keeps; [`Mirror::clear_in_flight`] consumes it,
    /// so clearing never allocates. Returns `None` if the media is already being mirrored —
    /// the existing mark belongs to the earlier caller, which clears it with its own key.
    fn mark_in_flight(&self, prefix: &str, id: &str) -> Option<(String, String)> {
        let key = (prefix.to_string(), id.to_string());
        self.in_flight.lock().insert(key.clone()).then_some(key)
    }

    /// Removes media from the in-flight set.
    ///
    /// Consumes the owned key that [`Mirror::mark_in_flight`] returned (or a clone of it), so
    /// the removal hashes once and touches no allocator. The set holds at most one entry per
    /// media being mirrored right now — one for every queued or running download, which the
    /// download manager bounds — so hashing into it is trivially cheap.
    fn clear_in_flight(in_flight: &InFlight, key: (String, String)) {
        let (prefix, id) = key;
        in_flight.lock().remove(&(prefix, id));
    }

    /// Returns the download manager, starting it on first use.
    ///
    /// The host starts the manager during startup with [`Mirror::start`], so normally it is
    /// already initialized here. The lazy path remains for mirrors used without that startup
    /// call, so the first request still works instead of being dropped with only a warning.
    fn download_manager(&self) -> Option<&DownloadManager> {
        self.manager.get_or_init(|| self.build_manager()).as_ref()
    }

    /// Mirrors the media at `url` unless it has already been mirrored or a mirror of it is
    /// already in progress.
    ///
    /// If the media has already been mirrored, its public link is returned. Otherwise the download
    /// is submitted to the download manager, which calls `on_mirrored` with the public link once
    /// it has been downloaded and uploaded, and `None` is returned.
    ///
    /// The id is a [`MediaId`], so it is already safe for file names, object keys and URL
    /// fragments; the download manager validates it again before it reaches the file system.
    ///
    /// # Errors
    ///
    /// Returns an error if it could not be checked whether the media has already been mirrored.
    pub async fn ensure_mirrored<F>(
        &self,
        prefix: &str,
        public_url_base: &Url,
        url: &str,
        id: &MediaId,
        on_mirrored: F,
    ) -> Result<Option<String>, MirrorError>
    where
        F: FnOnce(String) + Send + 'static,
    {
        if self.is_mirrored(prefix, id.as_str()).await? {
            let link = public_url_for(public_url_base, id.as_str());
            debug!(media.id = %id, media.link = %link, "media has already been mirrored");

            return Ok(Some(link));
        }

        let Some(in_flight_key) = self.mark_in_flight(prefix, id.as_str()) else {
            debug!(media.id = %id, "media is already being mirrored");

            return Ok(None);
        };

        let Some(manager) = self.download_manager() else {
            warn!(media.id = %id, "download manager is not running");
            // This path holds only borrows, so it consumes the mark's key directly; no copy
            // is made on the way to the removal.
            Self::clear_in_flight(&self.in_flight, in_flight_key);

            return Ok(None);
        };

        let in_flight = Arc::clone(&self.in_flight);
        let url = url.to_string();
        let public_url_base = public_url_base.clone();

        // The download is asynchronous and outlives this handler, so the download's span is
        // opened now, while the requester's span is current: it is a child of the requester, so
        // the download stays in the message's trace even though the handler returns (and its
        // span closes) first. Opening it here rather than when the download starts also keeps
        // the requester's span from staying open until a queued download finishes.
        let span = tracing::info_span!(
            parent: tracing::Span::current(),
            "media.download",
            media.id = %id
        );

        let request = DownloadRequest {
            url,
            id: in_flight_key.1.clone(),
            prefix: in_flight_key.0.clone(),
            public_url_base,
            on_finish: Box::new(move |result| {
                // The in-flight mark is always cleared, also on failure, so that a later link
                // triggers a new attempt. Consuming the mark's key keeps the removal
                // allocation-free. Errors are logged by the download manager.
                Self::clear_in_flight(&in_flight, in_flight_key);

                if let Ok(link) = result {
                    on_mirrored(link);
                }
            }),
            span,
        };

        if let Err(request) = manager.submit(request) {
            error!(
                media.id = %request.id,
                "could not submit the download, the manager is not running"
            );

            // The rejected request owns its key parts; consume them.
            Self::clear_in_flight(&self.in_flight, (request.prefix, request.id));
        }

        Ok(None)
    }
}
