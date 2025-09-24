//! Client interface for querying the danish dictionary service

use std::time::Duration;

use reqwest::{ClientBuilder, redirect::Policy};

use crate::{DictionaryDocument, Error};

const QUERY_URL: &str = "https://ws.dsl.dk/ddo/query";
const QUERY_WORD_PARAM: &str = "q";

#[derive(Debug)]
pub struct Client {
    client: reqwest::Client,
}

impl Client {
    /// Constructs a new client for interacting with the dictionary service.
    ///
    /// # Panics
    ///
    /// Panics if unable to construct a HTTP client. Refer to [`Client::try_new`] for more information.
    #[must_use]
    pub fn new() -> Client {
        Client::try_new().expect("could not construct http client")
    }

    /// Attempts to construct a new client for interacting with the dictionary service.
    ///
    /// # Errors
    ///
    /// This function returns [`Error::BuildClient`] when it fails to construct a HTTP client.
    ///
    /// Refer to [`ClientBuilder::build`] to see why it may fail.
    pub fn try_new() -> Result<Client, Error> {
        let client = ClientBuilder::new()
            .gzip(true)
            .redirect(Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(Error::BuildClient)?;

        Ok(Self::with_client(client))
    }

    /// Constructs a dictionary client that uses the given http client.
    #[must_use]
    pub const fn with_client(client: reqwest::Client) -> Client {
        Client { client }
    }

    /// Queries the dictionary for the given word.
    pub async fn query(&self, word: &str) -> Result<DictionaryDocument, Error> {
        let request = self
            .client
            .get(QUERY_URL)
            .query(&[(QUERY_WORD_PARAM, word)]);
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
