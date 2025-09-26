//! A client for querying the Danish dictionary web service (ordnet.dk).
//!
//! This module provides a high-level async interface for making requests to the dictionary and
//! parsing the results into structured data.

use std::time::Duration;

use reqwest::{ClientBuilder, redirect::Policy};

use crate::{DictionaryDocument, Error};

/// The base URL of the dictionary's service.
const BASE_URL: &str = "https://ws.dsl.dk";
/// The relative path of the query endpoint.
const QUERY_PATH: &str = "/ddo/query";
/// The name of the query parameter used to specify the word to look up.
const QUERY_WORD_PARAM: &str = "q";

/// An asynchronous client for the Danish Dictionary (Den Danske Ordbog).
///
/// This client handles the construction of HTTP requests, sending them to the dictionary service,
/// and parsing the HTML response.
#[derive(Debug)]
pub struct Client {
    /// The base URL of the service endpoint.
    base_url: String,
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
    pub fn new() -> Client {
        Client::try_new().expect("could not construct http client")
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
    pub fn try_new() -> Result<Client, Error> {
        let client = ClientBuilder::new()
            .gzip(true)
            .redirect(Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(Error::BuildClient)?;

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
    pub fn with_client(client: reqwest::Client) -> Client {
        let base_url = String::from(BASE_URL);

        Client { base_url, client }
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
    pub async fn query(&self, word: &str) -> Result<DictionaryDocument, Error> {
        let url = format!("{base_url}{QUERY_PATH}", base_url = self.base_url);
        let request = self.client.get(url).query(&[(QUERY_WORD_PARAM, word)]);
        let response = request.send().await.map_err(Error::Request)?;

        match response.error_for_status() {
            Ok(response) => {
                let body = response.text().await.map_err(Error::Request)?;

                DictionaryDocument::from_html(&body)
            }
            Err(err) => Err(Error::Request(err)),
        }
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

    #[test]
    fn test_with_client() {
        let http_client = reqwest::Client::new();
        let _ = Client::with_client(http_client);
    }
}
