//! Configuration for the shared media mirror.

use std::{path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

/// Configuration for the shared media mirror, from the `[mirror]` configuration section.
///
/// Unlike the plugin sections, unknown keys here are rejected (`deny_unknown_fields`): this is a
/// top-level section shared by all media plugins, and a typo'd or removed key is a hard
/// configuration mistake worth failing fast on.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MirrorConfig {
    /// The directory that downloads are buffered in.
    ///
    /// When unset, a random subdirectory of the system temporary directory is used and removed
    /// when the bot shuts down. Prefer a dedicated directory outside the system temporary
    /// directory: its cleanup services may remove in-progress downloads. The directory is owned
    /// by the process and must not be shared between concurrently running instances.
    pub download_dir: Option<PathBuf>,
    /// The maximum size of a file to download, as passed to `yt-dlp`.
    ///
    /// Files that report a larger size upfront are skipped.
    pub max_filesize: String,
    /// The maximum duration of a download before it gets killed.
    #[serde(with = "humantime_serde")]
    pub download_timeout: Duration,
    /// The maximum number of downloads that run concurrently.
    pub max_concurrent_downloads: usize,
    /// The command used to run `yt-dlp`.
    ///
    /// Falls back to the `ZETA_YTDLP_COMMAND` environment variable, and to `yt-dlp` when
    /// neither is set.
    pub ytdlp_command: Option<String>,
    /// The S3 access key id used for mirroring.
    ///
    /// Falls back to the `S3_ACCESS_KEY_ID` environment variable when unset. Mirroring is
    /// disabled when the S3 configuration is incomplete.
    pub s3_access_key_id: Option<String>,
    /// The S3 secret access key used for mirroring.
    ///
    /// Falls back to the `S3_SECRET_ACCESS_KEY` environment variable when unset.
    pub s3_secret_access_key: Option<String>,
    /// The bucket that mirrored media is uploaded to.
    ///
    /// Falls back to the `S3_BUCKET_NAME` environment variable when unset.
    pub s3_bucket_name: Option<String>,
    /// The S3 region.
    ///
    /// Falls back to the `S3_REGION` environment variable, and to `auto` when neither is set.
    /// `auto` is only meaningful for S3-compatible endpoints (such as R2 or MinIO) — against AWS
    /// proper, a real region should be set.
    pub s3_region: Option<String>,
    /// The endpoint URL for S3-compatible services.
    ///
    /// Falls back to the `S3_ENDPOINT` environment variable when unset. Empty values count as
    /// unset, so a blank endpoint does not override the environment or the AWS default.
    pub s3_endpoint: Option<String>,
    /// The maximum time to wait for a connection to the S3 endpoint.
    ///
    /// Bounds the failure of an unreachable endpoint instead of leaving the plugin that mirrors
    /// the media waiting indefinitely.
    #[serde(with = "humantime_serde")]
    pub s3_connect_timeout: Duration,
    /// The maximum time a single S3 request may take, including large uploads.
    ///
    /// Generous by default because it covers uploading a whole video; it exists to bound a
    /// stalled transfer, not to time out healthy uploads.
    #[serde(with = "humantime_serde")]
    pub s3_request_timeout: Duration,
}

impl Default for MirrorConfig {
    fn default() -> Self {
        Self {
            download_dir: None,
            max_filesize: "500M".to_string(),
            download_timeout: Duration::from_mins(10),
            max_concurrent_downloads: 4,
            ytdlp_command: None,
            s3_access_key_id: None,
            s3_secret_access_key: None,
            s3_bucket_name: None,
            s3_region: None,
            s3_endpoint: None,
            s3_connect_timeout: Duration::from_secs(10),
            s3_request_timeout: Duration::from_mins(5),
        }
    }
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
}
