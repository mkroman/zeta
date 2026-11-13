//! Extraction of a redirect target from a response.
//!
//! [`redirect_location`] sends a request through [`crate::send`], so the `http.client` span and
//! the failed-request log line cover the redirect probe, and returns the `Location` header as a
//! URL.

use reqwest::header::LOCATION;
use url::{ParseError, Url};

use crate::RequestError;
use crate::client::send;

/// The reasons [`redirect_location`] cannot return a URL.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RedirectError {
    /// The request could not be sent.
    #[error("request error: {0}")]
    Request(#[from] RequestError),
    /// The response did not carry a `Location` header, so it did not redirect.
    #[error("the response did not contain a location header")]
    MissingLocation,
    /// The `Location` header is not a valid header value (e.g. it is not visible ASCII).
    #[error("the response location header contains invalid encoding")]
    LocationEncoding(#[source] reqwest::header::ToStrError),
    /// The `Location` header is not a valid absolute URL.
    #[error("the response location header is not a valid url")]
    LocationUrl(#[source] ParseError),
}

/// Sends `request` and returns the URL in its `Location` header.
///
/// The request goes through [`send`], so the `http.client` span covers the probe and a failed
/// send is logged there. The header is read from the first response, so the request must come
/// from a client with redirects disabled — as [`crate::builder`] builds it; a client that
/// follows redirects would have consumed the header already.
///
/// # Errors
///
/// Returns [`RedirectError::Request`] if the request fails, [`RedirectError::MissingLocation`]
/// if the response has no `Location` header, and the encoding or URL variant if the header
/// cannot be used as a URL.
pub async fn redirect_location(request: reqwest::RequestBuilder) -> Result<Url, RedirectError> {
    let response = send(request).await?;
    let location = response
        .headers()
        .get(LOCATION)
        .ok_or(RedirectError::MissingLocation)?
        .to_str()
        .map_err(RedirectError::LocationEncoding)?;

    Url::parse(location).map_err(RedirectError::LocationUrl)
}
