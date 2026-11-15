//! Bounding and retrying S3 requests.
//!
//! A transient failure — a timeout, a connection failure or a server error — is retried with an
//! increasing delay for at most [`MAX_SEND_ATTEMPTS`] attempts. The presence check bounds its
//! whole retry chain with [`HEAD_TIMEOUT`], so an unresponsive endpoint cannot hold an upload.

use std::future::Future;
use std::time::Duration;

use reqwest::StatusCode;

use super::Error;

/// The maximum number of attempts for a request before giving up on transient failures.
const MAX_SEND_ATTEMPTS: u32 = 3;

/// The delay before the first retry of a transient failure; it doubles for each subsequent retry.
const RETRY_DELAY: Duration = Duration::from_millis(250);

/// The maximum time the presence check [`S3::object_exists`](super::S3::object_exists) may take,
/// including its retries.
///
/// A `HEAD` runs once per file before the upload, so an unresponsive endpoint must not hold the
/// upload for the generous per-request timeout, let alone across every retry of it.
pub(super) const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Sends a request built by `build`, retrying transient failures with a backoff.
pub(super) async fn send_with_retry<F, Fut>(mut build: F) -> Result<reqwest::Response, Error>
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
const fn is_retryable_error(error: &Error) -> bool {
    match error {
        Error::Request(error) => error.is_timeout() || error.is_connect() || error.is_request(),
        _ => false,
    }
}

/// Returns an error unless the response has a successful status.
pub(super) async fn ensure_success(response: reqwest::Response) -> Result<(), Error> {
    let status = response.status();

    if status.is_success() {
        return Ok(());
    }

    let body = response.text().await.unwrap_or_default();

    Err(Error::Status { status, body })
}
