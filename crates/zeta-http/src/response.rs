//! Status classification and JSON/text response parsing.
//!
//! [`parse_response`] and [`parse_text`] share one status-handling path and one set of log
//! lines, so a direct text read gets the same diagnostics as a JSON one. [`get_json`],
//! [`get_json_or_404`] and [`get_text`] pair them with [`send`], whose client span covers the
//! request and the body read.

use serde::de::DeserializeOwned;
use tracing::{Instrument, Span, debug, error, warn};
use zeta_url::RedactedUrl;
use zeta_utils::text::Truncatable;

use crate::RequestError;
use crate::client::{ClientSpan, send};
use crate::json;

/// The errors produced when sending a request and parsing its JSON response.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The request could not be sent, or the response body could not be read.
    ///
    /// Wraps the error in a [`RequestError`], which redacts its URL: these APIs carry their key
    /// in the query string, and the error reaches `Display` (and `Debug`) in plugin and
    /// dispatcher logs — and, through a plugin's error message, the channel.
    #[error("request error: {0}")]
    Request(#[from] RequestError),
    /// The server responded with a non-success status code, e.g. `404 Not Found`.
    #[error("{status}")]
    Status {
        /// The response status.
        status: reqwest::StatusCode,
        /// The error response body, read so callers can diagnose the failure (e.g. an API
        /// error message inside it).
        body: String,
    },
    /// The response body could not be parsed as JSON.
    #[error("could not deserialize response: {0}")]
    Deserialize(#[source] json::Error),
}

/// The number of characters of an error response body that reaches the logs: enough to read
/// the API's own message, bounded so an HTML error page cannot fill a log line.
pub const LOGGED_BODY_LENGTH: usize = 512;

/// Logs the status of a non-success response, with the already-truncated body when it could be
/// read. `record_error_type` is false when the body read already recorded the transport failure
/// on the span — overwriting it with the status code would lose the classification.
fn log_status(status: reqwest::StatusCode, body: Option<&str>, record_error_type: bool) {
    let not_found = status == reqwest::StatusCode::NOT_FOUND;

    if !not_found && record_error_type {
        Span::current().record("error.type", status.as_u16().to_string().as_str());
    }

    match (not_found, status.is_server_error(), body) {
        (true, _, Some(body)) => debug!(http.response.body = %body, "http response not found"),
        (true, _, None) => debug!("http response not found"),
        (false, true, Some(body)) => {
            error!(http.response.body = %body, "http response carries an error status");
        }
        (false, true, None) => error!("http response carries an error status"),
        (false, false, Some(body)) => {
            warn!(http.response.body = %body, "http response carries an error status");
        }
        (false, false, None) => warn!("http response carries an error status"),
    }
}

/// Wraps a failed body read as an [`ApiError`] and records its [`error.type`] on the response
/// span this read happens inside.
///
/// [`error.type`]: https://opentelemetry.io/docs/specs/semconv/registry/attributes/error/
fn body_error(error: reqwest::Error) -> ApiError {
    let error = RequestError::from(error);

    Span::current().record("error.type", error.error_type());

    ApiError::Request(error)
}

/// Reads the body of `response`, reporting a failed read as an [`ApiError::Request`].
async fn read_body(response: reqwest::Response) -> Result<String, ApiError> {
    response.text().await.map_err(body_error)
}

/// Reads the body of `response` inside the client span the request opened, logs the status, and
/// hands a successful body to `finish`.
///
/// Both [`parse_response`] and [`parse_text`] go through this, so the status handling and the
/// log lines are shared; `finish` runs inside the span, so a parse failure records `error.type`
/// on it. [`send`] stores its span in the response, so the body read stays in the span that
/// covers the request: one span per HTTP call, with no gap between the headers and the body.
/// A response that did not come through [`send`] (a caller that built it itself) gets a
/// synthesized client span from its own attributes.
async fn parse<T, F>(response: reqwest::Response, finish: F) -> Result<T, ApiError>
where
    F: FnOnce(String) -> Result<T, ApiError>,
{
    let span = response.extensions().get::<ClientSpan>().map_or_else(
        || {
            tracing::info_span!(
                "http.client",
                otel.kind = "client",
                url.full = %RedactedUrl::from_url(response.url()),
                server.address = %response.url().host_str().unwrap_or_default(),
                server.port = response.url().port_or_known_default(),
                http.response.status_code = response.status().as_u16(),
                error.type = tracing::field::Empty,
            )
        },
        |client| client.0.clone(),
    );

    async move {
        let status = response.status();

        if !status.is_success() {
            // Report the status even when its body cannot be read: the status is known before
            // the read, and without this line the response would reach the logs only as span
            // attributes. `body_error` records `error.type` first, so the event carries it too.
            let body = match read_body(response).await {
                Ok(body) => body,
                Err(error) => {
                    log_status(status, None, false);

                    return Err(error);
                }
            };
            let logged = body.truncate_within(LOGGED_BODY_LENGTH, "…");

            log_status(status, Some(&logged), true);

            return Err(ApiError::Status { status, body });
        }

        debug!("http response received");

        finish(read_body(response).await?)
    }
    .instrument(span)
    .await
}

/// Parses the response's JSON body into `T`.
///
/// Non-success statuses are reported as [`ApiError::Status`], carrying the response body; the
/// response is only parsed as JSON when the status is a success.
///
/// The body read and the parse run inside the HTTP client span the request opened —
/// <https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-client-span> — which
/// carries `otel.kind = "client"`, `http.request.method`, `url.full` (through [`RedactedUrl`],
/// since these APIs carry their key in the query string), `server.address`, `server.port` and
/// `http.response.status_code`, plus `error.type` when something goes wrong. The same span
/// reports the response: a `404` at debug level (it is the routine answer
/// [`parse_response_or_404`] turns into `not_found`), every other non-success status at error
/// or warn level with the truncated body, which is the only place that body reaches the logs.
/// [`parse_text`] shares the path, so text responses get the same attributes and lines.
///
/// # Errors
///
/// Returns an [`ApiError`] if the request fails, the response status is not a success, or the
/// body cannot be parsed as JSON.
pub async fn parse_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ApiError> {
    parse(response, |text| {
        json::from_str(&text).map_err(|error| {
            Span::current().record("error.type", "invalid_json");

            ApiError::Deserialize(error)
        })
    })
    .await
}

/// Reads the body of a successful response as text.
///
/// Non-success statuses are reported as [`ApiError::Status`], carrying the response body; the
/// body is only returned for a success. The status is logged, and the HTTP client attributes
/// recorded, the same way [`parse_response`] does it, so text responses get the same
/// diagnostics as JSON ones.
///
/// # Errors
///
/// Returns an [`ApiError`] if the status is not a success or the body cannot be read.
pub async fn parse_text(response: reqwest::Response) -> Result<String, ApiError> {
    parse(response, Ok).await
}

/// Parses a JSON response like [`parse_response`], mapping a `404` status to `not_found`.
///
/// The remaining [`ApiError`]s are converted into `E`.
///
/// # Errors
///
/// Returns `not_found` if the response status is `404 Not Found`; any other [`parse_response`]
/// error is converted into `E`.
pub async fn parse_response_or_404<T: DeserializeOwned, E: From<ApiError>>(
    response: reqwest::Response,
    not_found: E,
) -> Result<T, E> {
    match parse_response(response).await {
        Ok(value) => Ok(value),
        Err(ApiError::Status {
            status: reqwest::StatusCode::NOT_FOUND,
            ..
        }) => Err(not_found),
        Err(error) => Err(error.into()),
    }
}

/// Sends `request` and parses the JSON body of the successful response into `T`.
///
/// Combines [`send`] and [`parse_response`]: a failed send is logged once by [`send`] and
/// reported as [`ApiError::Request`], so callers write one statement instead of two.
///
/// # Errors
///
/// Returns an [`ApiError`] if the request fails, the response status is not a success, or the
/// body cannot be parsed as JSON.
pub async fn get_json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T, ApiError> {
    let response = send(request).await?;

    parse_response(response).await
}

/// Sends `request` and parses the JSON body like [`get_json`], mapping a `404` status to
/// `not_found`.
///
/// # Errors
///
/// Returns `not_found` if the response status is `404 Not Found`; any other failure is
/// converted into `E`.
pub async fn get_json_or_404<T: DeserializeOwned, E: From<ApiError>>(
    request: reqwest::RequestBuilder,
    not_found: E,
) -> Result<T, E> {
    let response = send(request)
        .await
        .map_err(|error| E::from(ApiError::from(error)))?;

    parse_response_or_404(response, not_found).await
}

/// Sends `request` and reads the body of the successful response as text.
///
/// Combines [`send`] and [`parse_text`]: a non-success status is reported as
/// [`ApiError::Status`], so callers cannot accidentally use an error page as content.
///
/// # Errors
///
/// Returns an [`ApiError`] if the request fails, the status is not a success, or the body cannot
/// be read.
pub async fn get_text(request: reqwest::RequestBuilder) -> Result<String, ApiError> {
    let response = send(request).await?;

    parse_text(response).await
}
