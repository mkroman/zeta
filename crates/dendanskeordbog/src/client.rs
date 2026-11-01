//! A client for querying the Danish dictionary web service (ordnet.dk).
//!
//! This module provides a high-level async interface for making requests to the dictionary and
//! parsing the results into structured data.

use std::time::Duration;

use reqwest::ClientBuilder;
use reqwest::redirect::Policy;

use crate::{DictionaryDocument, Error, RequestError};

/// Wraps `error` in a [`RequestError`], whose URL keeps no credential from the query string.
///
/// With the `log` feature the failed request is logged once with its redacted URL and error
/// type: the caller reports the short rendering, so this is the only line that names the
/// request.
fn request_error(error: reqwest::Error) -> RequestError {
    let error = RequestError::from(error);

    #[cfg(feature = "log")]
    if let Some(url) = error.url() {
        tracing::debug!(
            url.full = %url,
            error.type = error.error_type(),
            exception.message = %error.full(),
            "request failed"
        );
    }

    error
}

/// Parses a dictionary page on a blocking thread.
///
/// Parsing a full page is synchronous CPU work (`Html::parse_document`); on the blocking pool
/// it cannot stall the async executor while it runs. A failed task — e.g. a panic in the HTML
/// parser — is reported as [`Error::Parse`].
async fn parse_on_blocking_thread(
    parse: impl FnOnce() -> Result<DictionaryDocument, Error> + Send + 'static,
) -> Result<DictionaryDocument, Error> {
    tokio::task::spawn_blocking(parse)
        .await
        .map_err(|error| Error::Parse(error.to_string()))?
}

/// The URL of the dictionary's query endpoint.
const QUERY_URL: &str = "https://ws.dsl.dk/ddo/query";
/// The name of the query parameter used to specify the word to look up.
const QUERY_WORD_PARAM: &str = "q";

/// An asynchronous client for the Danish Dictionary (Den Danske Ordbog).
///
/// This client handles the construction of HTTP requests, sending them to the dictionary service,
/// and parsing the HTML response.
#[derive(Debug)]
pub struct Client {
    /// The underlying [`reqwest::Client`] used for making HTTP requests.
    client: reqwest::Client,
}

impl Client {
    /// Constructs a new `Client` with default settings.
    ///
    /// This method provides a convenient way to create a client. It configures default gzip
    /// support, a 30-second timeout, and disables redirects.
    ///
    /// # Panics
    ///
    /// Panics if the underlying HTTP client cannot be built. This can happen in environments with
    /// misconfigured network or TLS dependencies. For a non-panicking version, see
    /// [`Client::try_new`].
    #[must_use]
    pub fn new() -> Self {
        Self::try_new().expect("could not construct http client")
    }

    /// Attempts to construct a new `Client` with default settings.
    ///
    /// This is the fallible version of [`Client::new`]. It configures the client with gzip
    /// support, a 30-second timeout, and disables redirects.
    ///
    /// # Errors
    ///
    /// Returns an [`Error::BuildClient`] if the underlying `reqwest` client fails to build. See
    /// [`ClientBuilder::build`] for more details on potential failures.
    pub fn try_new() -> Result<Self, Error> {
        let client = ClientBuilder::new()
            .gzip(true)
            .redirect(Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| Error::BuildClient(error.into()))?;

        Ok(Self::with_client(client))
    }

    /// Constructs a `Client` using a pre-configured `reqwest::Client`.
    ///
    /// This is useful if you want to share an HTTP client between multiple services or require
    /// custom configuration (e.g., proxies, custom headers).
    ///
    /// # Arguments
    ///
    /// * `client` - An existing `reqwest::Client` instance.
    #[must_use]
    pub const fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Queries the dictionary for a specific word and returns the parsed result.
    ///
    /// This function performs the entire process of sending a request, awaiting the response, and
    /// parsing the HTML body into a [`DictionaryDocument`].
    ///
    /// # Arguments
    ///
    /// * `word` - The word to look up in the dictionary (e.g., "hest").
    ///
    /// # Errors
    ///
    /// This function can fail in several ways, returning an [`Error`]:
    /// - [`Error::Request`]: If the HTTP request fails due to network issues,
    ///   a timeout, or if the server returns a non-successful status code (e.g., 404, 500).
    /// - [`Error::MissingElement`]: If the response body is received but the HTML
    ///   is malformed or does not match the expected structure, preventing parsing.
    /// - [`Error::Parse`]: If the blocking parse task fails, e.g. because the HTML parser
    ///   panicked.
    pub async fn query(&self, word: &str) -> Result<DictionaryDocument, Error> {
        let request = self
            .client
            .get(QUERY_URL)
            .query(&[(QUERY_WORD_PARAM, word)]);
        let response = request
            .send()
            .await
            .map_err(|error| Error::Request(request_error(error)))?;

        if !response.status().is_success() {
            return Err(Error::Status(response.status()));
        }

        let body = response
            .text()
            .await
            .map_err(|error| Error::Request(request_error(error)))?;

        parse_on_blocking_thread(move || DictionaryDocument::from_html(body)).await
    }
}

impl Default for Client {
    /// Creates a default `Client` instance.
    ///
    /// This is equivalent to calling [`Client::new`].
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_the_response_body_on_a_blocking_thread() {
        let html = zeta_test_support::fixture!("queries/hest.html");

        let document = parse_on_blocking_thread(move || DictionaryDocument::from_html(html))
            .await
            .expect("parsed document");

        assert_eq!(document.entries.len(), 1);
        assert_eq!(document.entries[0].head.keyword, "hest");
    }

    #[tokio::test]
    async fn failed_parse_tasks_are_reported_as_parse_errors() {
        let error = parse_on_blocking_thread(|| panic!("parser panicked"))
            .await
            .expect_err("a panicking parse task should fail");

        assert!(matches!(&error, Error::Parse(_)), "{error:?}");
        assert!(
            error
                .to_string()
                .starts_with("could not parse the dictionary page:")
        );
    }

    #[tokio::test]
    async fn parser_errors_keep_their_own_variant() {
        // A page that parses but lacks the expected structure is the parser's error, not a
        // failed task.
        let html = r#"<html><body><span class="ar"></span></body></html>"#;

        let error = parse_on_blocking_thread(move || DictionaryDocument::from_html(html))
            .await
            .expect_err("a malformed entry should fail");

        assert!(matches!(&error, Error::MissingElement(_)), "{error:?}");
    }
}
