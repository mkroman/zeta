//! S3 client used for mirroring.
//!
//! The client is split by concern: [`signing`] builds the AWS Signature Version 4 headers for a
//! request, [`request`] issues the signed `HEAD` and `PUT` requests, [`retry`] bounds transient
//! failures, and [`file`] prepares an upload's body (its SHA-256 hash and content type). This
//! module owns the configuration, the [`Error`] type and the [`S3`] client itself.

mod file;
mod request;
mod retry;
mod signing;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::Path;

use aws_sigv4::sign::v4;
use reqwest::{Client, StatusCode};
use secrecy::ExposeSecret;
use tracing::{Span, instrument};
use url::Url;

use zeta_http::RequestError;

use crate::config::MirrorConfig;

/// Errors that can occur while talking to S3.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// A required configuration value is missing.
    #[error("{0}")]
    MissingConfig(String),
    /// The configured S3 endpoint is invalid.
    #[error("invalid s3 endpoint: {0}")]
    InvalidEndpoint(url::ParseError),
    /// An I/O error occurred.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The HTTP request failed.
    #[error("request failed: {0}")]
    Request(#[from] RequestError),
    /// The request could not be signed.
    #[error("could not sign request: {0}")]
    Signing(#[from] aws_sigv4::http_request::SigningError),
    /// The signing parameters were invalid.
    #[error("invalid signing parameters: {0}")]
    SigningParams(#[from] v4::signing_params::BuildError),
    /// S3 returned an unsuccessful response.
    #[error("s3 request failed with status {status}: {body}")]
    Status {
        /// The HTTP status code.
        status: StatusCode,
        /// The response body.
        body: String,
    },
}

impl Error {
    /// The low-cardinality [`error.type`] value for this failure, for a span or log field.
    ///
    /// [`error.type`]: https://opentelemetry.io/docs/specs/semconv/registry/attributes/error/
    const fn error_type(&self) -> &'static str {
        match self {
            Self::MissingConfig(_) => "s3_config",
            Self::InvalidEndpoint(_) => "s3_endpoint",
            Self::Io(_) => "s3_io",
            Self::Request(_) => "s3_request",
            Self::Signing(_) | Self::SigningParams(_) => "s3_signing",
            Self::Status { .. } => "s3_status",
        }
    }
}

/// Records the [`error.type`] of a failed S3 operation on the current span.
///
/// [`error.type`]: https://opentelemetry.io/docs/specs/semconv/registry/attributes/error/
fn record_error<T>(result: &Result<T, Error>) {
    if let Err(error) = result {
        Span::current().record("error.type", error.error_type());
    }
}

/// Resolves a secret from the configuration, falling back to the environment variable `name`.
///
/// The configured value takes precedence when it is set and non-empty; otherwise the value of
/// the `name` environment variable is used, so secrets can stay out of `config.toml`. Empty
/// values count as unset on both sides.
///
/// This mirrors `zeta_plugin::resolve_secret`: `zeta-mirror` cannot depend on `zeta-plugin`,
/// and both funnel into `zeta_utils::settings`, so keep the two in step.
///
/// # Errors
///
/// Returns [`Error::MissingConfig`] when neither the configured value nor the environment
/// variable is set.
fn resolve_secret(value: Option<&str>, name: &str) -> Result<String, Error> {
    zeta_utils::settings::resolve_optional_setting(value, name).ok_or_else(|| {
        Error::MissingConfig(format!(
            "neither the s3 setting nor environment variable `{name}` is set"
        ))
    })
}

/// Client for uploading mirrored files to an S3-compatible bucket.
#[derive(Clone)]
pub struct S3 {
    /// The HTTP client used for requests.
    client: Client,
    /// The S3 access key id.
    access_key_id: String,
    /// The S3 secret access key.
    secret_access_key: String,
    /// The name of the bucket to upload to.
    bucket: String,
    /// The region to sign requests for.
    region: String,
    /// The endpoint URL for S3-compatible services, when configured.
    endpoint: Option<Url>,
}

// Manual implementation so the secret access key is never printed.
impl fmt::Debug for S3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3")
            .field("access_key_id", &self.access_key_id)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl S3 {
    /// Creates a client from the given mirror configuration, falling back to the `S3_*`
    /// environment variables.
    ///
    /// # Errors
    ///
    /// Returns an error if a required value is missing or a configured value is invalid.
    pub fn try_new(config: &MirrorConfig) -> Result<Self, Error> {
        let access_key_id = resolve_secret(config.s3_access_key_id.as_deref(), "S3_ACCESS_KEY_ID")?;
        let secret_access_key = resolve_secret(
            config
                .s3_secret_access_key
                .as_ref()
                .map(ExposeSecret::expose_secret),
            "S3_SECRET_ACCESS_KEY",
        )?;
        let bucket = resolve_secret(config.s3_bucket_name.as_deref(), "S3_BUCKET_NAME")?;
        let region = zeta_utils::settings::resolve_optional_setting(
            config.s3_region.as_deref(),
            "S3_REGION",
        )
        .unwrap_or_else(|| "auto".to_owned());

        let endpoint = zeta_utils::settings::resolve_optional_setting(
            config.s3_endpoint.as_deref(),
            "S3_ENDPOINT",
        )
        .map(|endpoint| Url::parse(&endpoint))
        .transpose()
        .map_err(Error::InvalidEndpoint)?;

        // Signed requests must not follow redirects: replaying `Authorization` and the signature
        // at a redirect target cannot work, and an unresponsive endpoint must not hold the
        // plugin task — or a download slot — forever.
        let client = Client::builder()
            .connect_timeout(config.s3_connect_timeout)
            .timeout(config.s3_request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| Error::Request(RequestError::from(error)))?;

        Ok(Self {
            client,
            access_key_id,
            secret_access_key,
            bucket,
            region,
            endpoint,
        })
    }

    /// Returns whether an object with the given key exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the object could not be checked.
    #[instrument(
        name = "aws.s3.object_exists",
        skip_all,
        fields(
            otel.kind = "client",
            aws.s3.key = %key,
            http.request.method = "HEAD",
            error.type = tracing::field::Empty,
        )
    )]
    pub async fn object_exists(&self, key: &str) -> Result<bool, Error> {
        let result = self.head_object(key).await;

        record_error(&result);

        result
    }

    /// Uploads the file at the given `path` using the given object `key`.
    ///
    /// The file is streamed to the bucket, and is expected not to change during the upload.
    ///
    /// # Errors
    ///
    /// Returns an error if the file could not be read or uploaded.
    #[instrument(
        name = "aws.s3.upload",
        skip_all,
        fields(
            otel.kind = "client",
            aws.s3.key = %key,
            file.path = %path.display(),
            http.request.method = "PUT",
            error.type = tracing::field::Empty,
        )
    )]
    pub async fn upload_file(&self, path: &Path, key: &str) -> Result<(), Error> {
        let result = self.put_file(path, key).await;

        record_error(&result);

        result
    }
}

#[cfg(test)]
impl S3 {
    /// Creates a client that points at an unreachable local endpoint, for tests that never
    /// talk to S3.
    #[must_use]
    pub(crate) fn for_test() -> Self {
        Self::with_endpoint("http://127.0.0.1:9")
    }

    /// Creates a client that points at the given local endpoint, for tests that never talk
    /// to a real S3.
    ///
    /// # Panics
    ///
    /// Panics if `endpoint` is not a valid URL.
    #[must_use]
    pub(crate) fn with_endpoint(endpoint: &str) -> Self {
        Self {
            client: Client::new(),
            access_key_id: "test-access-key".to_string(),
            secret_access_key: "test-secret-key".to_string(),
            bucket: "test".to_string(),
            region: "auto".to_string(),
            endpoint: Some(Url::parse(endpoint).expect("valid endpoint")),
        }
    }
}
