//! Mirroring of media to S3.
//!
//! Downloads media with `yt-dlp`, uploads it to an S3-compatible bucket and links to the public
//! copy. The service is shared by every plugin that mirrors media (such as the tiktok and reddit
//! plugins) and is configured through the top-level `[mirror]` configuration section or the
//! `S3_*` environment variables; when the configuration is incomplete, plugins degrade to their
//! non-mirroring behavior.
//!
//! The actual mirroring is managed by a long-running download manager task that downloads media
//! (streaming progress status back), uploads it to the bucket in a bounded upload pipeline and
//! notifies the requester with the public link. Downloads happen inside temporary directories
//! that are owned by the manager and removed when the download finishes; directories left behind
//! by a killed or crashed process are removed on startup.
//!
//! Plugins access the shared mirror through a [`MirrorHandle`],
//! which carries the key prefix and the public URL base used for their links, and is resolved
//! from their own configuration.

mod config;
mod handle;
mod manager;
mod s3;
mod tempdir;
mod ytdlp;

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use tracing::{debug, error, warn};
use url::Url;

use manager::{DownloadManager, DownloadRequest};
use s3::S3;
use tempdir::{STALE_DOWNLOAD_MAX_AGE, cleanup_stale_downloads, tempdir_builder};
use ytdlp::{YtDlp, YtDlpOptions};

pub use config::MirrorConfig;
pub use handle::MirrorHandle;
pub use s3::Error as MirrorError;

use zeta_url::is_identifier;

/// The media currently being mirrored, keyed by `(prefix, id)`.
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
        let s3 = S3::new(config)?;

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

    /// Returns `false` if the media is already being mirrored, otherwise marks it as in-flight.
    fn mark_in_flight(&self, prefix: &str, id: &str) -> bool {
        zeta_utils::sync::lock(&self.in_flight).insert((prefix.to_string(), id.to_string()))
    }

    /// Removes media from an in-flight set.
    fn clear_in_flight(in_flight: &InFlight, prefix: &str, id: &str) {
        zeta_utils::sync::lock(in_flight).remove(&(prefix.to_string(), id.to_string()));
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
    /// # Errors
    ///
    /// Returns an error if it could not be checked whether the media has already been mirrored.
    pub async fn ensure_mirrored<F>(
        &self,
        prefix: &str,
        public_url_base: &Url,
        url: &str,
        id: &str,
        on_mirrored: F,
    ) -> Result<Option<String>, MirrorError>
    where
        F: FnOnce(String) + Send + 'static,
    {
        // The id ends up in file names, object keys and the public link fragment; ids that are
        // not safe are never mirrored. It is validated here (before the fast-path check) and
        // again by the download manager before it reaches the file system.
        if !is_identifier(id, "_-") {
            warn!(media.id = %id, "ignoring mirror request with an unsafe id");

            return Ok(None);
        }

        if self.is_mirrored(prefix, id).await? {
            let link = public_url_for(public_url_base, id);
            debug!(media.id = %id, %link, "media has already been mirrored");

            return Ok(Some(link));
        }

        if !self.mark_in_flight(prefix, id) {
            debug!(media.id = %id, "media is already being mirrored");

            return Ok(None);
        }

        let Some(manager) = self.download_manager() else {
            warn!(media.id = %id, "download manager is not running");
            Self::clear_in_flight(&self.in_flight, prefix, id);

            return Ok(None);
        };

        let in_flight = Arc::clone(&self.in_flight);
        let url = url.to_string();
        let id = id.to_string();
        let prefix = prefix.to_string();
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
            id: id.clone(),
            prefix: prefix.clone(),
            public_url_base,
            on_finish: Box::new(move |result| {
                // The in-flight mark is always cleared, also on failure, so that a later link
                // triggers a new attempt. Errors are logged by the download manager.
                Self::clear_in_flight(&in_flight, &prefix, &id);

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

            Self::clear_in_flight(&self.in_flight, &request.prefix, &request.id);
        }

        Ok(None)
    }
}

/// Returns the object key for the given file name under `prefix`.
#[must_use]
pub(crate) fn object_key(prefix: &str, filename: &str) -> String {
    match prefix.trim_end_matches('/') {
        "" => filename.to_string(),
        prefix => format!("{prefix}/{filename}"),
    }
}

/// Returns the public URL for the given id, using the given base URL.
#[must_use]
pub(crate) fn public_url_for(base: &Url, id: &str) -> String {
    let mut url = base.clone();
    url.set_fragment(Some(id));
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_key() {
        assert_eq!(object_key("", "123.mp4"), "123.mp4");
        assert_eq!(object_key("reddit", "123.mp4"), "reddit/123.mp4");
        assert_eq!(
            object_key("~meta/reddit", "123.mp4"),
            "~meta/reddit/123.mp4"
        );
        assert_eq!(
            object_key("~meta/reddit/", "123.mp4"),
            "~meta/reddit/123.mp4"
        );
    }

    #[test]
    fn test_public_url_for() {
        let base = Url::parse("https://pub.rwx.im/reddit").unwrap();

        assert_eq!(
            public_url_for(&base, "pxtf7mx2xqzg1"),
            "https://pub.rwx.im/reddit#pxtf7mx2xqzg1"
        );
    }

    /// Returns a mirror that never talks to S3 and has no manager running.
    fn mirror_for_test() -> Mirror {
        Mirror::new(S3::for_test(), YtDlp::with_command("yt-dlp"), None, 2)
    }

    #[test]
    fn test_in_flight_tracking() {
        let mirror = mirror_for_test();

        assert!(mirror.mark_in_flight("reddit", "123"));
        // Media that is already being mirrored is not marked again.
        assert!(!mirror.mark_in_flight("reddit", "123"));
        assert!(mirror.mark_in_flight("reddit", "456"));
        // The same id under a different prefix is a different piece of media.
        assert!(mirror.mark_in_flight("tiktok", "456"));

        Mirror::clear_in_flight(&mirror.in_flight, "reddit", "123");
        assert!(mirror.mark_in_flight("reddit", "123"));
    }

    #[tokio::test]
    async fn test_start_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let download_dir = dir.path().join("downloads");
        let mirror = Mirror::new(
            S3::for_test(),
            YtDlp::with_command("false"),
            Some(download_dir.clone()),
            2,
        );

        assert!(mirror.manager.get().is_none());
        assert!(mirror.start(), "the manager should start");
        assert!(mirror.manager.get().is_some());

        // A second start is a no-op: rebuilding the manager would recreate the removed
        // directory.
        std::fs::remove_dir(&download_dir).unwrap();
        assert!(mirror.start());
        assert!(
            !download_dir.exists(),
            "the second start rebuilt the download manager"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_start_disables_mirroring_when_the_directory_cannot_be_created() {
        let mirror = Mirror::new(
            S3::for_test(),
            YtDlp::with_command("false"),
            Some(PathBuf::from("/dev/null/downloads")),
            2,
        );

        assert!(!mirror.start(), "the manager should not start");
        assert!(matches!(mirror.manager.get(), Some(None)));
        // The failure is cached, so a later call does not retry.
        assert!(!mirror.start());
    }

    /// Returns a mirror whose S3 endpoint answers every request with a 404, so that the
    /// fast-path check finds no mirror, together with the server that must outlive it.
    async fn mirror_with_missing_objects() -> (Mirror, zeta_test_support::wiremock::MockServer) {
        let server = zeta_test_support::not_found_server().await;
        let endpoint = format!("http://{}", server.address());
        let mirror = Mirror::new(
            S3::with_endpoint(&endpoint),
            // `false` fails every download immediately, so the test never reaches the network.
            YtDlp::with_command("false"),
            None,
            2,
        );

        (mirror, server)
    }

    #[tokio::test]
    async fn test_ensure_mirrored_rejects_unsafe_id() {
        let mirror = mirror_for_test();
        let base = Url::parse("https://pub.rwx.im/reddit").unwrap();

        // The request is rejected before any S3 request is made.
        let result = mirror
            .ensure_mirrored(
                "reddit",
                &base,
                "https://v.redd.it/../evil",
                "../evil",
                |_| {},
            )
            .await
            .expect("the rejection should not be an error");

        assert!(result.is_none());
        // The media was not marked as in-flight, so a later mirror can be requested.
        assert!(mirror.mark_in_flight("reddit", "../evil"));
    }

    #[tokio::test]
    async fn test_ensure_mirrored_starts_the_download_manager() {
        let (mirror, _server) = mirror_with_missing_objects().await;
        let base = Url::parse("https://pub.rwx.im/reddit").unwrap();

        // The manager is not started by a `loaded` hook anymore: the first request must start
        // it, or a URL delivered before `loaded` runs would be dropped.
        assert!(mirror.manager.get().is_none());

        let result = mirror
            .ensure_mirrored("reddit", &base, "https://v.redd.it/123", "123", |_| {})
            .await
            .expect("the fast-path check should succeed");

        assert!(result.is_none());
        assert!(
            mirror.download_manager().is_some(),
            "the first mirror request should start the download manager"
        );
    }
}
