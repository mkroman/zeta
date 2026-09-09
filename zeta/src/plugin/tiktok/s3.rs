//! S3 client used for mirroring videos.

use std::path::Path;

use aws_sdk_s3::{
    config::{BehaviorVersion, Credentials, Region},
    error::SdkError,
    primitives::{ByteStream, ByteStreamError},
};
use tracing::debug;
use url::Url;

use crate::plugin::prelude::ZetaError;

/// The default public URL that mirrored videos are linked with.
const DEFAULT_PUBLIC_URL_BASE: &str = "https://pub.rwx.im/tiktok";

/// The path prefix used for all object keys.
const KEY_ROOT: &str = "tiktok";

/// The name used to identify our credentials provider.
const CREDENTIALS_PROVIDER_NAME: &str = "zeta-tiktok-plugin";

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("{0}")]
    MissingConfig(#[from] ZetaError),
    #[error("invalid public url base: {0}")]
    InvalidPublicUrlBase(#[from] url::ParseError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not read file: {0}")]
    Read(#[from] ByteStreamError),
    #[error("s3 error: {0}")]
    S3(Box<aws_sdk_s3::Error>),
}

/// Client for uploading videos to an S3-compatible bucket.
#[derive(Clone, Debug)]
pub struct S3 {
    /// The S3 client.
    client: aws_sdk_s3::Client,
    /// The name of the bucket to upload to.
    bucket: String,
    /// The optional key prefix for all uploaded objects.
    prefix: Option<String>,
    /// The base URL used when linking to mirrored videos.
    public_url_base: Url,
}

impl S3 {
    /// Creates a client from the `S3_*` environment variables.
    ///
    /// Requires `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY` and `S3_BUCKET_NAME`; `S3_ENDPOINT`,
    /// `S3_REGION` and `S3_PREFIX` are optional. `S3_REGION` defaults to `"auto"`, which is only
    /// meaningful for S3-compatible endpoints (such as R2 or MinIO) — against AWS proper, a real
    /// region should be set.
    ///
    /// The base URL for public links can be set with `TIKTOK_PUBLIC_URL_BASE`. Links are built by
    /// appending the video id as a URL fragment, so the base must point at a viewer page that
    /// resolves the fragment — not directly at the bucket.
    ///
    /// # Errors
    ///
    /// Returns an error if a required environment variable is missing or a configured value is
    /// invalid.
    pub fn from_env() -> Result<Self, Error> {
        let access_key_id = require_env_string("S3_ACCESS_KEY_ID")?;
        let secret_access_key = require_env_string("S3_SECRET_ACCESS_KEY")?;
        let bucket = require_env_string("S3_BUCKET_NAME")?;
        let region = std::env::var("S3_REGION").unwrap_or_else(|_| "auto".to_string());
        let endpoint = std::env::var("S3_ENDPOINT").ok();
        let prefix = std::env::var("S3_PREFIX").ok();
        let public_url_base = match std::env::var("TIKTOK_PUBLIC_URL_BASE") {
            Ok(value) => Url::parse(&value)?,
            Err(_) => Url::parse(DEFAULT_PUBLIC_URL_BASE)?,
        };

        let mut config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region))
            .credentials_provider(Credentials::new(
                access_key_id,
                secret_access_key,
                None,
                None,
                CREDENTIALS_PROVIDER_NAME,
            ));

        if let Some(endpoint) = endpoint {
            config = config.endpoint_url(endpoint).force_path_style(true);
        }

        Ok(Self {
            client: aws_sdk_s3::Client::from_conf(config.build()),
            bucket,
            prefix,
            public_url_base,
        })
    }

    /// Creates a client that points at an unreachable local endpoint, for tests that never talk
    /// to S3.
    #[cfg(test)]
    #[must_use]
    pub fn for_test() -> Self {
        Self::with_endpoint("http://127.0.0.1:9")
    }

    /// Creates a client that points at the given local endpoint, for tests that never talk to a
    /// real S3.
    #[cfg(test)]
    #[must_use]
    pub fn with_endpoint(endpoint: &str) -> Self {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .build();

        Self {
            client: aws_sdk_s3::Client::from_conf(config),
            bucket: "test".to_string(),
            prefix: None,
            public_url_base: Url::parse(DEFAULT_PUBLIC_URL_BASE).expect("valid default url"),
        }
    }

    /// Returns the object key for the given file name.
    #[must_use]
    pub fn key_for(&self, filename: &str) -> String {
        object_key(self.prefix.as_deref(), filename)
    }

    /// Returns the public, viewable URL for the given video id.
    #[must_use]
    pub fn public_url(&self, video_id: &str) -> String {
        public_url_for(&self.public_url_base, video_id)
    }

    /// Returns whether an object with the given key exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the object could not be checked.
    pub async fn object_exists(&self, key: &str) -> Result<bool, Error> {
        debug!(%key, "checking if object exists");

        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError(service_error)) => {
                let not_found = service_error.err().is_not_found()
                    || service_error.raw().status().as_u16() == 404;

                if not_found {
                    Ok(false)
                } else {
                    Err(Error::S3(Box::new(aws_sdk_s3::Error::from(
                        SdkError::ServiceError(service_error),
                    ))))
                }
            }
            Err(err) => Err(Error::S3(Box::new(aws_sdk_s3::Error::from(err)))),
        }
    }

    /// Uploads the file at the given `path` using the given object `key`.
    ///
    /// The file is streamed to the bucket, and is expected not to change during the upload.
    ///
    /// # Errors
    ///
    /// Returns an error if the file could not be read or uploaded.
    pub async fn upload_file(&self, path: &Path, key: &str) -> Result<(), Error> {
        debug!(%key, path = %path.display(), "uploading file to s3");

        let body = ByteStream::from_path(path).await?;

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type_for(path))
            .body(body)
            .send()
            .await
            .map_err(|err| Error::S3(Box::new(aws_sdk_s3::Error::from(err))))?;

        Ok(())
    }
}

/// Returns the object key for the given file name.
#[must_use]
fn object_key(prefix: Option<&str>, filename: &str) -> String {
    match prefix {
        Some(prefix) if !prefix.is_empty() => {
            format!("{}/{KEY_ROOT}/{filename}", prefix.trim_end_matches('/'))
        }
        _ => format!("{KEY_ROOT}/{filename}"),
    }
}

/// Returns the public URL for the given video id, using the given base URL.
#[must_use]
fn public_url_for(base: &Url, video_id: &str) -> String {
    let mut url = base.clone();
    url.set_fragment(Some(video_id));
    url.to_string()
}

/// Returns the content type for the given path, based on its extension.
fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("mp4" | "mov" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        _ => "application/octet-stream",
    }
}

/// Reads a required environment variable, mapping the error into our error type.
fn require_env_string(name: &str) -> Result<String, Error> {
    crate::plugin::prelude::require_env(name).map_err(Error::MissingConfig)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_key() {
        assert_eq!(object_key(None, "123.mp4"), "tiktok/123.mp4");
        assert_eq!(object_key(Some(""), "123.mp4"), "tiktok/123.mp4");
        assert_eq!(object_key(Some("~meta"), "123.mp4"), "~meta/tiktok/123.mp4");
        assert_eq!(
            object_key(Some("~meta/"), "123.mp4"),
            "~meta/tiktok/123.mp4"
        );
    }

    #[test]
    fn test_public_url_for() {
        let base = Url::parse(DEFAULT_PUBLIC_URL_BASE).unwrap();

        assert_eq!(
            public_url_for(&base, "7541501431543532814"),
            "https://pub.rwx.im/tiktok#7541501431543532814"
        );
    }
}
