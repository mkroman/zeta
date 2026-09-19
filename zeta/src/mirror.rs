//! Mirroring of media to S3.
//!
//! Downloads media with `yt-dlp`, uploads it to an S3-compatible bucket and links to the public
//! copy. The service is shared by every plugin that mirrors media (such as the tiktok and reddit
//! plugins) and is configured through the top-level `[mirror]` configuration section or the
//! `S3_*` environment variables; when the configuration is incomplete, plugins degrade to their
//! non-mirroring behavior.
//!
//! The actual mirroring is managed by [`DownloadManager`](crate::mirror::DownloadManager), a
//! long-running task that downloads media (streaming progress status back), uploads it to the
//! bucket and notifies the requester with the public link. Downloads happen inside temporary
//! directories that are owned by the manager and removed when the download finishes; directories
//! left behind by a killed or crashed process are removed on startup.
//!
//! Plugins access the shared mirror through a [`MirrorTarget`](crate::mirror::MirrorTarget),
//! which carries the key prefix and the public URL base used for their links, and is resolved
//! from their own configuration.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tracing::{debug, error, warn};
use url::Url;

pub mod manager;
pub mod s3;
pub mod ytdlp;

pub use manager::{DownloadManager, DownloadRequest};
pub use s3::S3;
pub use ytdlp::{YtDlp, YtDlpOptions};

use crate::context::Context;

/// Configuration for the shared media mirror, from the `[mirror]` configuration section.
///
/// Unlike the plugin sections, unknown keys here are rejected (`deny_unknown_fields`): this is a
/// top-level section shared by all media plugins, and a typo'd or removed key is a hard
/// configuration mistake worth failing fast on.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorConfig {
    /// The directory that downloads are buffered in.
    ///
    /// When unset, a random subdirectory of the system temporary directory is used and removed
    /// when the bot shuts down. Prefer a dedicated directory outside the system temporary
    /// directory: its cleanup services may remove in-progress downloads. The directory is owned
    /// by the process and must not be shared between concurrently running instances.
    #[serde(default)]
    pub download_dir: Option<PathBuf>,
    /// The maximum size of a file to download, as passed to `yt-dlp`.
    ///
    /// Files that report a larger size upfront are skipped.
    #[serde(default = "default_max_filesize")]
    pub max_filesize: String,
    /// The maximum duration of a download before it gets killed.
    #[serde(default = "default_download_timeout", with = "humantime_serde")]
    pub download_timeout: Duration,
    /// The maximum number of downloads that run concurrently.
    #[serde(default = "default_max_concurrent_downloads")]
    pub max_concurrent_downloads: usize,
    /// The command used to run `yt-dlp`.
    ///
    /// Falls back to the `ZETA_YTDLP_COMMAND` environment variable, and to `yt-dlp` when
    /// neither is set.
    #[serde(default)]
    pub ytdlp_command: Option<String>,
    /// The S3 access key id used for mirroring.
    ///
    /// Falls back to the `S3_ACCESS_KEY_ID` environment variable when unset. Mirroring is
    /// disabled when the S3 configuration is incomplete.
    #[serde(default)]
    pub s3_access_key_id: Option<String>,
    /// The S3 secret access key used for mirroring.
    ///
    /// Falls back to the `S3_SECRET_ACCESS_KEY` environment variable when unset.
    #[serde(default)]
    pub s3_secret_access_key: Option<String>,
    /// The bucket that mirrored media is uploaded to.
    ///
    /// Falls back to the `S3_BUCKET_NAME` environment variable when unset.
    #[serde(default)]
    pub s3_bucket_name: Option<String>,
    /// The S3 region.
    ///
    /// Falls back to the `S3_REGION` environment variable, and to `auto` when neither is set.
    #[serde(default)]
    pub s3_region: Option<String>,
    /// The endpoint URL for S3-compatible services.
    ///
    /// Falls back to the `S3_ENDPOINT` environment variable when unset.
    #[serde(default)]
    pub s3_endpoint: Option<String>,
}

impl Default for MirrorConfig {
    fn default() -> Self {
        Self {
            download_dir: None,
            max_filesize: default_max_filesize(),
            download_timeout: default_download_timeout(),
            max_concurrent_downloads: default_max_concurrent_downloads(),
            ytdlp_command: None,
            s3_access_key_id: None,
            s3_secret_access_key: None,
            s3_bucket_name: None,
            s3_region: None,
            s3_endpoint: None,
        }
    }
}

/// Returns the default maximum file size.
fn default_max_filesize() -> String {
    "500M".to_string()
}

/// Returns the default download timeout.
const fn default_download_timeout() -> Duration {
    Duration::from_mins(10)
}

/// Returns the default maximum number of concurrent downloads.
const fn default_max_concurrent_downloads() -> usize {
    4
}

/// Mirrors media: downloads it with `yt-dlp` and uploads it to S3.
///
/// A single instance is shared by all plugins; it is created from [`MirrorConfig`] at startup and
/// published to [`Context::shared`].
#[derive(Clone)]
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
    in_flight: Arc<Mutex<HashSet<(String, String)>>>,
    /// The download manager, started by [`Mirror::start_downloads`]; `None` when the download
    /// directory could not be created, disabling mirroring.
    manager: OnceLock<Option<DownloadManager>>,
}

impl Mirror {
    /// Creates a mirror from the shared `[mirror]` configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the S3 configuration is incomplete or invalid.
    pub fn from_config(config: &MirrorConfig) -> Result<Self, s3::Error> {
        let ytdlp = YtDlp::new(YtDlpOptions {
            command: config.ytdlp_command.clone(),
            max_filesize: config.max_filesize.clone(),
            download_timeout: config.download_timeout,
        });
        let s3 = S3::new(s3::S3Config {
            access_key_id: config.s3_access_key_id.clone(),
            secret_access_key: config.s3_secret_access_key.clone(),
            bucket: config.s3_bucket_name.clone(),
            region: config.s3_region.clone(),
            endpoint: config.s3_endpoint.clone(),
        })?;

        Ok(Self::new(
            s3,
            ytdlp,
            config.download_dir.clone(),
            config.max_concurrent_downloads,
        ))
    }

    /// Creates a mirror using the given S3 client and `yt-dlp` runner.
    #[must_use]
    pub fn new(s3: S3, ytdlp: YtDlp, download_dir: Option<PathBuf>, max_concurrent: usize) -> Self {
        Self {
            s3,
            ytdlp,
            download_dir,
            max_concurrent,
            in_flight: Arc::default(),
            manager: OnceLock::new(),
        }
    }

    /// Starts the download manager task.
    ///
    /// Must be called before [`Mirror::ensure_mirrored`], when a plugin using the mirror is
    /// loaded. Subsequent calls are no-ops.
    ///
    /// If the download directory cannot be created, mirroring stays disabled: the outcome is
    /// cached and later calls do not retry.
    pub fn start_downloads(&self) {
        let _ = self.manager.get_or_init(|| {
            // When no download directory is configured, a random subdirectory of the system
            // temporary directory is used and kept alive for as long as the manager runs; if it
            // cannot be created, mirroring stays disabled rather than writing into the system
            // temporary directory itself. Otherwise the configured directory is created if
            // missing. Either way, temporary directories left behind by a previous run are
            // removed first.
            let (download_dir, base_dir) = self.download_dir.clone().map_or_else(
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
                                error = %err,
                                "could not create a download directory, mirroring is disabled"
                            );
                        })
                        .ok()
                },
                |dir| {
                    if let Err(err) = std::fs::create_dir_all(&dir) {
                        error!(
                            path = %dir.display(),
                            error = %err,
                            "could not create the download directory, mirroring is disabled"
                        );

                        return None;
                    }

                    Some((dir, None))
                },
            )?;

            Some(DownloadManager::start(
                self.ytdlp.clone(),
                self.s3.clone(),
                download_dir,
                base_dir,
                self.max_concurrent,
            ))
        });
    }

    /// Returns whether media with the given id has already been mirrored under `prefix`.
    ///
    /// This is a fast-path check that assumes the common `.mp4` container; media that was
    /// mirrored under a different container is only caught by the exact-key check performed by
    /// the download manager after a new download.
    async fn is_mirrored(&self, prefix: &str, id: &str) -> Result<bool, s3::Error> {
        let key = object_key(prefix, &format!("{id}.mp4"));

        self.s3.object_exists(&key).await
    }

    /// Returns `false` if the media is already being mirrored, otherwise marks it as in-flight.
    fn mark_in_flight(&self, prefix: &str, id: &str) -> bool {
        self.in_flight
            .lock()
            .expect("in-flight lock is poisoned")
            .insert((prefix.to_string(), id.to_string()))
    }

    /// Removes media from the in-flight set.
    fn clear_in_flight(&self, prefix: &str, id: &str) {
        self.in_flight
            .lock()
            .expect("in-flight lock is poisoned")
            .remove(&(prefix.to_string(), id.to_string()));
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
    ) -> Result<Option<String>, s3::Error>
    where
        F: FnOnce(String) + Send + 'static,
    {
        // The id ends up in file names, object keys and the public link fragment; ids that are
        // not safe are never mirrored. It is validated here (before the fast-path check) and
        // again by the download manager before it reaches the file system.
        if !is_safe_id(id) {
            warn!(%id, "ignoring mirror request with an unsafe id");

            return Ok(None);
        }

        if self.is_mirrored(prefix, id).await? {
            let link = public_url_for(public_url_base, id);
            debug!(%id, %link, "media has already been mirrored");

            return Ok(Some(link));
        }

        if !self.mark_in_flight(prefix, id) {
            debug!(%id, "media is already being mirrored");

            return Ok(None);
        }

        let Some(manager) = self.manager.get().and_then(Option::as_ref) else {
            warn!(%id, "download manager is not running");
            self.clear_in_flight(prefix, id);

            return Ok(None);
        };

        let mirror = self.clone();
        let url = url.to_string();
        let id = id.to_string();
        let prefix = prefix.to_string();
        let public_url_base = public_url_base.clone();

        let request = DownloadRequest {
            url,
            id: id.clone(),
            prefix: prefix.clone(),
            public_url_base,
            on_finish: Box::new(move |result| {
                // The in-flight mark is always cleared, also on failure, so that a later link
                // triggers a new attempt. Errors are logged by the download manager.
                mirror.clear_in_flight(&prefix, &id);

                if let Ok(link) = result {
                    on_mirrored(link);
                }
            }),
        };

        if let Err(request) = manager.submit(request) {
            error!(
                id = %request.id,
                "could not submit the download, the manager is not running"
            );

            self.clear_in_flight(&request.prefix, &request.id);
        }

        Ok(None)
    }
}

/// A plugin's handle to the shared [`Mirror`].
///
/// Carries the key prefix the plugin's mirrored files are uploaded under and the base URL its
/// public links are built from.
#[derive(Clone)]
pub struct MirrorTarget {
    /// The shared mirror.
    mirror: Arc<Mirror>,
    /// The key prefix for the plugin's uploads.
    prefix: String,
    /// The base URL for the plugin's public links.
    public_url_base: Url,
}

impl MirrorTarget {
    /// Creates a target for the given mirror.
    #[must_use]
    pub const fn new(mirror: Arc<Mirror>, prefix: String, public_url_base: Url) -> Self {
        Self {
            mirror,
            prefix,
            public_url_base,
        }
    }

    /// Resolves a target from a plugin's configuration.
    ///
    /// Returns `None` when the shared mirror is unavailable or the configured public URL base is
    /// invalid, in which case the plugin should degrade to its non-mirroring behavior.
    #[must_use]
    pub fn resolve(
        mirror: Option<Arc<Mirror>>,
        prefix: Option<&str>,
        prefix_env: &str,
        prefix_default: &str,
        public_url_base: Option<&str>,
        public_url_base_env: &str,
        public_url_base_default: &str,
    ) -> Option<Self> {
        let public_url_base = resolve_public_url_base(
            public_url_base,
            public_url_base_env,
            public_url_base_default,
        )?;
        let prefix = resolve_prefix(prefix, prefix_env, prefix_default);
        let mirror = mirror?;

        Some(Self::new(mirror, prefix, public_url_base))
    }

    /// Starts the download manager task.
    pub fn start_downloads(&self) {
        self.mirror.start_downloads();
    }

    /// Mirrors the media at `url` unless it has already been mirrored or a mirror of it is
    /// already in progress; see [`Mirror::ensure_mirrored`].
    ///
    /// # Errors
    ///
    /// Returns an error if it could not be checked whether the media has already been mirrored.
    pub async fn ensure_mirrored<F>(
        &self,
        url: &str,
        id: &str,
        on_mirrored: F,
    ) -> Result<Option<String>, s3::Error>
    where
        F: FnOnce(String) + Send + 'static,
    {
        self.mirror
            .ensure_mirrored(&self.prefix, &self.public_url_base, url, id, on_mirrored)
            .await
    }
}

/// Resolves the public URL base from a plugin's configuration.
///
/// The configured value wins, then the environment variable, then the default. Returns `None` and
/// logs a warning when the resolved value is not a valid URL.
#[must_use]
pub fn resolve_public_url_base(setting: Option<&str>, env: &str, default: &str) -> Option<Url> {
    let value = setting
        .map(str::to_string)
        .or_else(|| std::env::var(env).ok())
        .unwrap_or_else(|| default.to_string());

    match Url::parse(&value) {
        Ok(url) => Some(url),
        Err(error) => {
            warn!(%error, %value, "invalid public url base, mirroring is disabled");

            None
        }
    }
}

/// Resolves a key prefix from a plugin's configuration.
///
/// The configured value wins, then the environment variable, then the default.
#[must_use]
pub fn resolve_prefix(setting: Option<&str>, env: &str, default: &str) -> String {
    setting
        .map(str::to_string)
        .or_else(|| std::env::var(env).ok())
        .unwrap_or_else(|| default.to_string())
}

/// Constructs and publishes the shared mirror to [`Context::shared`], based on the `[mirror]`
/// configuration section.
///
/// Mirroring stays disabled when the configuration is incomplete or invalid.
pub(crate) fn publish_shared_mirror(ctx: &Context) {
    match Mirror::from_config(&ctx.config.mirror) {
        Ok(mirror) => {
            ctx.shared.publish(Arc::new(mirror));
        }
        Err(error) => {
            warn!(%error, "mirroring is disabled");
        }
    }
}

/// The filename prefix for our temporary download directories.
pub(crate) const TEMP_DIR_PREFIX: &str = "zeta-mirror-";

/// The maximum age of a download directory before it is considered stale during the sweep of the
/// shared system temporary directory.
///
/// The system temporary directory may hold the active download directories of other running
/// processes (including other instances of the bot), so only directories that have not been
/// touched recently are removed there.
const STALE_DOWNLOAD_MAX_AGE: Duration = Duration::from_hours(24);

/// Returns a builder for our temporary download directories.
///
/// Directories are private to the current user on unix, since downloaded media may be sensitive.
#[must_use]
pub(crate) fn tempdir_builder() -> tempfile::Builder<'static, 'static> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = tempfile::Builder::new().prefix(TEMP_DIR_PREFIX).clone();

    #[cfg(unix)]
    builder.permissions(std::fs::Permissions::from_mode(0o700));

    builder
}

/// Returns whether `id` is safe to use in file names and object keys: it must be non-empty and
/// consist of ASCII alphanumerics, `_` or `-`.
#[must_use]
pub(crate) fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Removes any temporary download directories left behind by a previous run.
///
/// Returns the number of directories removed. When `max_age` is given, only directories whose
/// modification time is older are removed; use it when sweeping a directory that may hold the
/// active download directories of other running processes (the system temporary directory). Left
/// `None` for directories owned by this process.
pub(super) fn cleanup_stale_downloads(base: &std::path::Path, max_age: Option<Duration>) -> usize {
    let Ok(entries) = std::fs::read_dir(base) else {
        return 0;
    };

    let mut removed = 0;

    for entry in entries.flatten() {
        if !is_temp_download_dir(&entry) {
            continue;
        }

        if let Some(max_age) = max_age
            && !is_older_than(&entry, max_age)
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

/// Whether `entry` is a temporary download directory left by a mirror download.
pub(super) fn is_temp_download_dir(entry: &std::fs::DirEntry) -> bool {
    entry
        .file_name()
        .to_string_lossy()
        .starts_with(TEMP_DIR_PREFIX)
        && entry.file_type().is_ok_and(|file_type| file_type.is_dir())
}

/// Writes `body` as an executable, uniquely named test script, returning its path.
#[cfg(test)]
pub(crate) fn write_test_script(name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script = std::env::temp_dir().join(format!("zeta-test-{name}-{}.sh", std::process::id()));
    std::fs::write(&script, body).unwrap();

    let mut permissions = std::fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).unwrap();

    script
}

/// Returns whether the entry was last modified longer than `age` ago.
///
/// Entries whose age cannot be determined (including ones with a modification time in the future)
/// are not considered old.
fn is_older_than(entry: &std::fs::DirEntry, age: Duration) -> bool {
    entry
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed > age)
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
    use std::path::Path;

    use super::*;

    #[test]
    fn default_config() {
        let config = MirrorConfig::default();

        assert!(config.download_dir.is_none());
        assert_eq!(config.max_filesize, "500M");
        assert_eq!(config.download_timeout, Duration::from_mins(10));
        assert_eq!(config.max_concurrent_downloads, 4);
        assert!(config.ytdlp_command.is_none());
        assert!(config.s3_access_key_id.is_none());
        assert!(config.s3_secret_access_key.is_none());
        assert!(config.s3_bucket_name.is_none());
        assert!(config.s3_region.is_none());
        assert!(config.s3_endpoint.is_none());
    }

    #[test]
    fn config_deserialize() {
        let config: MirrorConfig = serde_json::from_value(serde_json::json!({
            "download_dir": "/var/cache/zeta/mirror",
            "max_filesize": "100M",
            "download_timeout": "5m",
            "max_concurrent_downloads": 1,
            "ytdlp_command": "/usr/local/bin/yt-dlp",
            "s3_access_key_id": "access",
            "s3_secret_access_key": "secret",
            "s3_bucket_name": "bucket",
            "s3_region": "auto",
            "s3_endpoint": "https://example.com",
        }))
        .expect("could not deserialize config");

        assert_eq!(
            config.download_dir.as_deref(),
            Some(Path::new("/var/cache/zeta/mirror"))
        );
        assert_eq!(config.max_filesize, "100M");
        assert_eq!(config.download_timeout, Duration::from_mins(5));
        assert_eq!(config.max_concurrent_downloads, 1);
        assert_eq!(
            config.ytdlp_command.as_deref(),
            Some("/usr/local/bin/yt-dlp")
        );
        assert_eq!(config.s3_access_key_id.as_deref(), Some("access"));
        assert_eq!(config.s3_secret_access_key.as_deref(), Some("secret"));
        assert_eq!(config.s3_bucket_name.as_deref(), Some("bucket"));
        assert_eq!(config.s3_region.as_deref(), Some("auto"));
        assert_eq!(config.s3_endpoint.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn config_rejects_unknown_keys() {
        assert!(
            serde_json::from_value::<MirrorConfig>(serde_json::json!({ "s3_bucket_nam": "x" }))
                .is_err(),
            "typo'd key should be rejected"
        );
    }

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

    #[test]
    fn test_resolve_prefix() {
        assert_eq!(resolve_prefix(Some("~meta"), "X_PREFIX", "reddit"), "~meta");
        assert_eq!(resolve_prefix(None, "X_PREFIX", "reddit"), "reddit");
    }

    #[test]
    fn test_resolve_public_url_base() {
        let base = resolve_public_url_base(
            Some("https://pub.example.com/x"),
            "X_PUBLIC_URL_BASE",
            "https://pub.rwx.im/reddit",
        )
        .expect("configured base should be valid");

        assert_eq!(base.as_str(), "https://pub.example.com/x");

        let base = resolve_public_url_base(None, "X_PUBLIC_URL_BASE", "https://pub.rwx.im/reddit")
            .expect("default base should be valid");

        assert_eq!(base.as_str(), "https://pub.rwx.im/reddit");

        assert!(resolve_public_url_base(Some("not a url"), "X_PUBLIC_URL_BASE", "").is_none());
    }

    #[test]
    fn test_is_safe_id() {
        assert!(is_safe_id("123"));
        assert!(is_safe_id("pxtf7mx2xqzg1"));
        assert!(is_safe_id("a-b_c"));
        assert!(!is_safe_id(""));
        assert!(!is_safe_id("../evil"));
        assert!(!is_safe_id("a/b"));
        assert!(!is_safe_id("a b"));
        assert!(!is_safe_id("%(id)s"));
    }

    #[test]
    fn test_cleanup_stale_downloads_respects_age() {
        let base = tempfile::tempdir().unwrap();

        let fresh = base.path().join(format!("{TEMP_DIR_PREFIX}1234"));
        std::fs::create_dir(&fresh).unwrap();

        // A directory that was touched recently is not stale while a maximum age is given, but
        // is removed unconditionally when no age is given.
        cleanup_stale_downloads(base.path(), Some(STALE_DOWNLOAD_MAX_AGE));
        assert!(fresh.exists());

        cleanup_stale_downloads(base.path(), None);
        assert!(!fresh.exists());
    }

    /// Returns a mirror that never talks to S3 and has no manager running.
    fn mirror_for_test() -> Mirror {
        Mirror {
            s3: S3::for_test(),
            ytdlp: YtDlp::with_command("yt-dlp"),
            download_dir: None,
            max_concurrent: 2,
            in_flight: Arc::default(),
            manager: OnceLock::new(),
        }
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

        mirror.clear_in_flight("reddit", "123");
        assert!(mirror.mark_in_flight("reddit", "123"));
    }

    /// Runs a minimal S3 server that answers every request with a 404, so that the fast-path
    /// check finds no mirror. Returns the mirror pointed at the server and a handle for the
    /// server thread.
    fn mirror_with_missing_objects() -> (Mirror, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};

            let (mut stream, _) = listener.accept().unwrap();

            let mut buffer = [0; 1024];
            let _ = stream.read(&mut buffer);

            stream
                .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                .unwrap();
        });

        let mirror = Mirror {
            s3: S3::with_endpoint(&format!("http://{address}")),
            ytdlp: YtDlp::with_command("yt-dlp"),
            download_dir: None,
            max_concurrent: 2,
            in_flight: Arc::default(),
            manager: OnceLock::new(),
        };

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
    async fn test_ensure_mirrored_without_manager_clears_in_flight() {
        let (mirror, server) = mirror_with_missing_objects();
        let base = Url::parse("https://pub.rwx.im/reddit").unwrap();

        let result = mirror
            .ensure_mirrored("reddit", &base, "https://v.redd.it/123", "123", |_| {})
            .await
            .expect("the fast-path check should succeed");

        assert!(result.is_none());
        // The media must not remain marked as in-flight, so a later mirror can be requested.
        assert!(mirror.mark_in_flight("reddit", "123"));

        server.join().unwrap();
    }
}
