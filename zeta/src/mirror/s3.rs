//! S3 client used for mirroring.

use std::fmt;
use std::future::Future;
use std::path::Path;
use std::time::{Duration, SystemTime};

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings,
    UriPathNormalizationMode, sign,
};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use reqwest::{Body, Client, StatusCode};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tracing::debug;
use url::Url;

use zeta_plugin::Error as ZetaError;
use zeta_plugin::prelude::resolve_secret;

/// The name used to identify our credentials provider.
const CREDENTIALS_PROVIDER_NAME: &str = "zeta-mirror";

/// The size of the buffer used when hashing files.
const HASH_BUFFER_SIZE: usize = 64 * 1024;

/// The maximum number of attempts for a request before giving up on transient failures.
const MAX_SEND_ATTEMPTS: u32 = 3;

/// The delay before the first retry of a transient failure; it doubles for each subsequent retry.
const RETRY_DELAY: Duration = Duration::from_millis(250);

/// Configuration for the S3 mirror client.
///
/// Required values fall back to the `S3_*` environment variables when unset; optional values
/// fall back to their environment variables and then to defaults.
#[derive(Clone, Debug, Default)]
pub struct S3Config {
    /// The S3 access key id.
    ///
    /// Falls back to the `S3_ACCESS_KEY_ID` environment variable when unset.
    pub access_key_id: Option<String>,
    /// The S3 secret access key.
    ///
    /// Falls back to the `S3_SECRET_ACCESS_KEY` environment variable when unset.
    pub secret_access_key: Option<String>,
    /// The name of the bucket to upload to.
    ///
    /// Falls back to the `S3_BUCKET_NAME` environment variable when unset.
    pub bucket: Option<String>,
    /// The region to use.
    ///
    /// Falls back to the `S3_REGION` environment variable, and to `auto` when neither is set.
    /// `auto` is only meaningful for S3-compatible endpoints (such as R2 or MinIO) — against AWS
    /// proper, a real region should be set.
    pub region: Option<String>,
    /// The endpoint URL for S3-compatible services.
    ///
    /// Falls back to the `S3_ENDPOINT` environment variable when unset.
    pub endpoint: Option<String>,
}

/// Errors that can occur while talking to S3.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// A required configuration value is missing.
    #[error("{0}")]
    MissingConfig(#[from] ZetaError),
    /// The configured S3 endpoint is invalid.
    #[error("invalid s3 endpoint: {0}")]
    InvalidEndpoint(url::ParseError),
    /// An I/O error occurred.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The HTTP request failed.
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
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
    /// Creates a client from the given configuration, falling back to the `S3_*` environment
    /// variables.
    ///
    /// # Errors
    ///
    /// Returns an error if a required value is missing or a configured value is invalid.
    pub fn new(config: S3Config) -> Result<Self, Error> {
        let access_key_id = resolve_secret(config.access_key_id.as_deref(), "S3_ACCESS_KEY_ID")
            .map_err(Error::MissingConfig)?;
        let secret_access_key =
            resolve_secret(config.secret_access_key.as_deref(), "S3_SECRET_ACCESS_KEY")
                .map_err(Error::MissingConfig)?;
        let bucket = resolve_secret(config.bucket.as_deref(), "S3_BUCKET_NAME")
            .map_err(Error::MissingConfig)?;
        let region = crate::utils::resolve_setting(config.region.as_deref(), "S3_REGION", "auto");

        let endpoint = config
            .endpoint
            .or_else(|| std::env::var("S3_ENDPOINT").ok())
            .map(|endpoint| Url::parse(&endpoint))
            .transpose()
            .map_err(Error::InvalidEndpoint)?;

        Ok(Self {
            client: Client::builder().build()?,
            access_key_id,
            secret_access_key,
            bucket,
            region,
            endpoint,
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
    ///
    /// # Panics
    ///
    /// Panics if `endpoint` is not a valid URL.
    #[cfg(test)]
    #[must_use]
    pub fn with_endpoint(endpoint: &str) -> Self {
        Self {
            client: Client::new(),
            access_key_id: "test-access-key".to_string(),
            secret_access_key: "test-secret-key".to_string(),
            bucket: "test".to_string(),
            region: "auto".to_string(),
            endpoint: Some(Url::parse(endpoint).expect("valid endpoint")),
        }
    }

    /// Returns whether an object with the given key exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the object could not be checked.
    pub async fn object_exists(&self, key: &str) -> Result<bool, Error> {
        debug!(%key, "checking if object exists");

        let url = self.object_url(key)?;
        let headers = self.signing_headers("HEAD", &url, SignableBody::empty())?;

        let response = send_with_retry(|| async {
            let request = apply_headers(self.client.head(url.clone()), &headers);

            Ok(request.send().await?)
        })
        .await?;

        match response.status() {
            status if status.is_success() => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(Error::Status {
                status,
                body: String::new(),
            }),
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

        let length = tokio::fs::metadata(path).await?.len();
        let digest = sha256_file(path).await?;
        let url = self.object_url(key)?;
        let headers = self.signing_headers("PUT", &url, SignableBody::Precomputed(digest))?;

        let response = send_with_retry(|| async {
            let file = tokio::fs::File::open(path).await.map_err(Error::from)?;
            let request = apply_headers(
                self.client
                    .put(url.clone())
                    .header(CONTENT_LENGTH, length)
                    .header(CONTENT_TYPE, content_type_for(path))
                    .body(Body::from(file)),
                &headers,
            );

            Ok(request.send().await?)
        })
        .await?;

        ensure_success(response).await
    }

    /// Returns the URL for the object with the given key.
    fn object_url(&self, key: &str) -> Result<Url, Error> {
        self.endpoint
            .as_ref()
            .map_or_else(
                || {
                    Url::parse(&format!(
                        "https://{}.s3.{}.amazonaws.com/{}",
                        self.bucket, self.region, key
                    ))
                },
                |endpoint| {
                    Url::parse(&format!(
                        "{}/{}/{}",
                        endpoint.as_str().trim_end_matches('/'),
                        self.bucket,
                        key
                    ))
                },
            )
            .map_err(Error::InvalidEndpoint)
    }

    /// Signs a request for the given URL and body, returning the headers to apply to it.
    fn signing_headers(
        &self,
        method: &str,
        url: &Url,
        body: SignableBody<'_>,
    ) -> Result<Vec<(&'static str, String)>, Error> {
        let credentials = Credentials::new(
            self.access_key_id.as_str(),
            self.secret_access_key.as_str(),
            None,
            None,
            CREDENTIALS_PROVIDER_NAME,
        );
        let identity: Identity = credentials.into();
        let mut settings = SigningSettings::default();
        // S3 requires the payload hash to be part of the signature, must not have its URI path
        // normalized, and only single-encodes URI paths.
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        let params: aws_sigv4::http_request::SigningParams<'_> = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name("s3")
            .time(SystemTime::now())
            .settings(settings)
            .build()?
            .into();

        let request = SignableRequest::new(method, url.as_str(), std::iter::empty(), body)?;
        let (instructions, _signature) = sign(request, &params)?.into_parts();
        let (headers, _query) = instructions.into_parts();

        Ok(headers
            .into_iter()
            .map(|header| (header.name(), header.value().to_string()))
            .collect())
    }
}

/// Applies the signed `headers` to `request`.
fn apply_headers(
    mut request: reqwest::RequestBuilder,
    headers: &[(&'static str, String)],
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        request = request.header(*name, value.as_str());
    }

    request
}

/// Sends a request built by `build`, retrying transient failures with a backoff.
async fn send_with_retry<F, Fut>(mut build: F) -> Result<reqwest::Response, Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, Error>>,
{
    let mut attempt = 0;

    loop {
        attempt += 1;

        match build().await {
            Ok(response)
                if attempt >= MAX_SEND_ATTEMPTS || !is_retryable_status(response.status()) =>
            {
                return Ok(response);
            }
            Err(error) if attempt >= MAX_SEND_ATTEMPTS || !is_retryable_error(&error) => {
                return Err(error);
            }
            Ok(_) | Err(_) => {}
        }

        tokio::time::sleep(RETRY_DELAY * attempt).await;
    }
}

/// Whether the response status denotes a transient failure worth retrying.
fn is_retryable_status(status: StatusCode) -> bool {
    status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
}

/// Whether the error denotes a transient failure worth retrying.
fn is_retryable_error(error: &Error) -> bool {
    match error {
        Error::Request(error) => error.is_timeout() || error.is_connect() || error.is_request(),
        _ => false,
    }
}

/// Returns an error unless the response has a successful status.
async fn ensure_success(response: reqwest::Response) -> Result<(), Error> {
    let status = response.status();

    if status.is_success() {
        return Ok(());
    }

    let body = response.text().await.unwrap_or_default();

    Err(Error::Status { status, body })
}

/// Returns the lowercase hex-encoded SHA-256 digest of the file at `path`.
async fn sha256_file(path: &Path) -> Result<String, Error> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];

    loop {
        let read = file.read(&mut buffer).await?;

        if read == 0 {
            break;
        }

        hasher.update(&buffer[..read]);
    }

    Ok(const_hex::encode(hasher.finalize()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_url_path_style() {
        let s3 = S3::with_endpoint("http://localhost:9000");

        assert_eq!(
            s3.object_url("reddit/123.mp4").unwrap().as_str(),
            "http://localhost:9000/test/reddit/123.mp4"
        );
    }

    #[test]
    fn test_signing_headers_include_content_hash() {
        let s3 = S3::with_endpoint("http://localhost:9000");
        let url = s3.object_url("tiktok/123.mp4").unwrap();

        let headers = s3
            .signing_headers("HEAD", &url, SignableBody::empty())
            .expect("could not sign request");

        assert!(
            headers
                .iter()
                .any(|(name, _)| *name == "x-amz-content-sha256"),
            "signature must include the payload hash: {headers:?}"
        );
        assert!(
            headers.iter().any(|(name, _)| *name == "authorization"),
            "signature must include the authorization header: {headers:?}"
        );
    }

    #[test]
    fn test_precomputed_body_hash_matches_bytes() {
        let s3 = S3::with_endpoint("http://localhost:9000");
        let url = s3.object_url("tiktok/123.mp4").unwrap();
        let digest = const_hex::encode(Sha256::digest(b"hello"));

        let hash_of = |body: SignableBody<'_>| {
            s3.signing_headers("PUT", &url, body)
                .expect("could not sign request")
                .into_iter()
                .find(|(name, _)| *name == "x-amz-content-sha256")
                .map(|(_, value)| value)
                .expect("signature must include the payload hash")
        };

        assert_eq!(hash_of(SignableBody::Precomputed(digest.clone())), digest);
        assert_eq!(
            hash_of(SignableBody::Precomputed(digest)),
            hash_of(SignableBody::Bytes(b"hello"))
        );
    }
}
