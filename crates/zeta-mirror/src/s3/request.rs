//! The signed requests the client issues to the bucket.
//!
//! A presence check is a `HEAD` on the object's URL, bounded by [`HEAD_TIMEOUT`] across all of
//! its retries; an upload is a `PUT` that streams the file with its precomputed hash. Both
//! retry transient failures through [`send_with_retry`]. Object URLs are path-style, so they
//! work against S3-compatible endpoints as well as against AWS.

use std::path::Path;

use aws_sigv4::http_request::SignableBody;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use reqwest::{Body, StatusCode};
use tracing::debug;
use url::Url;

use super::file::{content_type_for, sha256_file};
use super::retry::{HEAD_TIMEOUT, ensure_success, send_with_retry};
use super::signing::apply_headers;
use super::{Error, S3};

impl S3 {
    /// Issues the signed `HEAD` request [`object_exists`](Self::object_exists) is built on.
    pub(super) async fn head_object(&self, key: &str) -> Result<bool, Error> {
        debug!("checking if object exists");

        let url = self.object_url(key)?;
        let headers = self.signing_headers("HEAD", &url, SignableBody::empty())?;

        // The timeout wraps the whole retry chain, not a single attempt, so a retrying endpoint
        // cannot extend the check past its bound.
        let response = tokio::time::timeout(
            HEAD_TIMEOUT,
            send_with_retry(|| async {
                let request = apply_headers(self.client.head(url.clone()), &headers);

                zeta_http::send(request).await.map_err(Error::from)
            }),
        )
        .await
        .map_err(|_| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "s3 head timed out",
            ))
        })??;

        match response.status() {
            status if status.is_success() => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(Error::Status {
                status,
                body: String::new(),
            }),
        }
    }

    /// Issues the signed `PUT` request [`upload_file`](Self::upload_file) is built on.
    pub(super) async fn put_file(&self, path: &Path, key: &str) -> Result<(), Error> {
        debug!("uploading file to s3");

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

            zeta_http::send(request).await.map_err(Error::from)
        })
        .await?;

        ensure_success(response).await
    }

    /// Returns the URL for the object with the given key.
    pub(super) fn object_url(&self, key: &str) -> Result<Url, Error> {
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
}
