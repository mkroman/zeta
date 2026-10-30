//! Client for the Kagi Search API.

use std::time::{Duration, Instant};

use reqwest::header::{
    ACCEPT, ACCEPT_LANGUAGE, CACHE_CONTROL, HeaderValue, PRAGMA, REFERER, SET_COOKIE,
    UPGRADE_INSECURE_REQUESTS,
};
use reqwest::redirect::Policy;
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Mutex;
use tracing::debug;

use crate::{
    BASE_URL, ClientOptions, Error, ImageResult, RequestError, SearchResult,
    parse::{
        KagiMessage, extract_nonce, parse_image_result_messages, parse_search_result_messages,
        parse_stream, url_with_query,
    },
};

/// Wraps `error` in a [`RequestError`], whose URL keeps no credential from the query string:
/// the login token is part of Kagi's query strings.
fn request_error(error: reqwest::Error) -> RequestError {
    RequestError::from(error)
}

/// The `Accept` header sent for document (navigation) requests.
const ACCEPT_DOCUMENT: HeaderValue =
    HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8");
/// The `Accept` header sent for server-sent event stream requests.
const ACCEPT_EVENT_STREAM: HeaderValue = HeaderValue::from_static("text/event-stream");

#[derive(Clone, Debug)]
struct Session {
    /// The nonce used for the first stream request of the session.
    nonce: Option<String>,
    /// When the nonce was created.
    created_at: Instant,
}

/// Client for searching with Kagi.
pub struct Client {
    /// HTTP client with a cookie jar.
    http: reqwest::Client,
    /// Kagi login token.
    token: SecretString,
    /// Session details.
    session: Mutex<Option<Session>>,
    /// The duration of a single session.
    session_duration: Duration,
    /// The `Accept-Language` header sent with requests.
    language: HeaderValue,
}

impl Session {
    fn is_valid(&self, session_duration: Duration) -> bool {
        self.created_at.elapsed() < session_duration
    }

    /// Returns the nonce on the first call and `None` afterwards, mirroring a browser which
    /// only sends the page's `sse_nonce` on its initial connection.
    const fn take_nonce(&mut self) -> Option<String> {
        self.nonce.take()
    }
}

impl Client {
    /// Constructs a new [`Client`] for searching with Kagi using the given session token and
    /// default options.
    ///
    /// The token is the value of the `kagi_session` cookie from an authenticated browser session.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client fails to build.
    pub fn with_token(token: impl Into<SecretString>) -> Self {
        Self::with_token_and_options(token, ClientOptions::default())
            .expect("could not build http client")
    }

    /// Constructs a new [`Client`] for searching with Kagi using the given session token and
    /// options.
    ///
    /// The token is the value of the `kagi_session` cookie from an authenticated browser session.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client fails to build.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidHeader`] if a configured header value is invalid.
    pub fn with_token_and_options(
        token: impl Into<SecretString>,
        options: ClientOptions,
    ) -> Result<Self, Error> {
        let client = reqwest::ClientBuilder::new()
            .cookie_store(true)
            .redirect(Policy::none())
            .timeout(options.timeout)
            .user_agent(options.user_agent)
            .build()
            .map_err(|error| Error::BuildClient(error.into()))?;

        Ok(Self {
            http: client,
            token: token.into(),
            session: Mutex::new(None),
            session_duration: options.session_duration,
            language: HeaderValue::from_str(&options.language)?,
        })
    }

    /// Attaches the session authorization header to the request, mirroring the browser which
    /// sends it with every request.
    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header("X-Kagi-Authorization", self.token.expose_secret())
    }

    /// Builds a request shaped like a browser document navigation.
    fn document_request(&self, url: &str) -> reqwest::RequestBuilder {
        self.authorize(self.http.get(url))
            .header(ACCEPT, ACCEPT_DOCUMENT)
            .header(ACCEPT_LANGUAGE, self.language.clone())
            .header("Sec-Fetch-Dest", HeaderValue::from_static("document"))
            .header("Sec-Fetch-Mode", HeaderValue::from_static("navigate"))
            .header("Sec-Fetch-Site", HeaderValue::from_static("none"))
            .header("Sec-Fetch-User", HeaderValue::from_static("?1"))
            .header(UPGRADE_INSECURE_REQUESTS, HeaderValue::from_static("1"))
    }

    /// Builds a request shaped like a browser `EventSource` connection.
    fn stream_request(&self, url: reqwest::Url) -> reqwest::RequestBuilder {
        self.authorize(self.http.get(url))
            .header(ACCEPT, ACCEPT_EVENT_STREAM)
            .header(ACCEPT_LANGUAGE, self.language.clone())
            .header("Sec-Fetch-Dest", HeaderValue::from_static("empty"))
            .header("Sec-Fetch-Mode", HeaderValue::from_static("cors"))
            .header("Sec-Fetch-Site", HeaderValue::from_static("same-origin"))
            .header(PRAGMA, HeaderValue::from_static("no-cache"))
            .header(CACHE_CONTROL, HeaderValue::from_static("no-cache"))
            .header("Priority", HeaderValue::from_static("u=4"))
    }

    async fn fetch_new_session_data(&self) -> Result<String, Error> {
        // Issue a request with the login token to receive session cookies. The token is part of
        // the query string, so the request is intentionally not logged here.
        let token_url = url_with_query("/search", &[("token", self.token.expose_secret())]);
        let req = self.document_request(token_url.as_str());
        debug!("requesting session cookies");

        let res = req
            .send()
            .await
            .map_err(|error| Error::RequestSession(request_error(error)))?;
        if !res.headers().contains_key(SET_COOKIE) {
            return Err(Error::SessionCookies);
        }

        // Request the main page to receive a nonce for the first stream request.
        debug!("requesting nonce");
        let req = self.document_request(BASE_URL);
        let res = req
            .send()
            .await
            .map_err(|error| Error::RequestNonce(request_error(error)))?;
        let body = res
            .text()
            .await
            .map_err(|error| Error::ReadNonce(request_error(error)))?;

        extract_nonce(&body).ok_or(Error::Nonce)
    }

    /// Returns the nonce to include with the next stream request, refreshing the session if
    /// necessary.
    ///
    /// The nonce is only returned once per session, mirroring a browser which sends the page's
    /// `sse_nonce` on its initial connection only.
    // `Option::map_or_else` is not viable here: the `None` arm has to store the refreshed
    // session back into the guard.
    #[allow(clippy::option_if_let_else)]
    async fn take_nonce(&self) -> Result<Option<String>, Error> {
        {
            let mut guard = self.session.lock().await;

            if let Some(session) = guard
                .as_mut()
                .filter(|session| session.is_valid(self.session_duration))
            {
                return Ok(session.take_nonce());
            }
        }

        debug!("session expired or missing, refreshing...");
        let nonce = self.fetch_new_session_data().await?;

        // Double check and use the current session if it was refreshed while we were fetching
        // the new one.
        let taken = {
            let mut guard = self.session.lock().await;

            if let Some(session) = guard
                .as_mut()
                .filter(|session| session.is_valid(self.session_duration))
            {
                session.take_nonce()
            } else {
                let mut session = Session {
                    nonce: Some(nonce),
                    created_at: Instant::now(),
                };
                let taken = session.take_nonce();
                *guard = Some(session);

                taken
            }
        };

        Ok(taken)
    }

    /// Fetches one of the socket stream endpoints (`search` or `images`) and returns the messages
    /// parsed from the buffered response body.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if a session could not be established, the stream request could not
    /// be sent, or the response could not be read.
    async fn fetch_stream(&self, endpoint: &str, query: &str) -> Result<Vec<KagiMessage>, Error> {
        let nonce = self.take_nonce().await?;
        let url = url_with_query(&format!("/socket/{endpoint}"), &[("q", query)]);
        let referer = url_with_query(&format!("/{endpoint}"), &[("q", query)]);
        let req = self.stream_request(url).header(REFERER, referer.as_str());
        let req = if let Some(nonce) = nonce {
            req.query(&[("nonce", nonce)])
        } else {
            req
        };

        debug!(%endpoint, "connecting to stream");
        let res = req
            .send()
            .await
            .map_err(|error| Error::StreamRequest(request_error(error)))?;
        let res = res
            .error_for_status()
            .map_err(|error| Error::StreamStatus(request_error(error)))?;
        let body = res
            .text()
            .await
            .map_err(|error| Error::StreamRequestBody(request_error(error)))?;

        Ok(parse_stream(&body))
    }

    /// Searches Kagi with the given query and returns the parsed search results.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if a session could not be established, the stream request could not
    /// be sent, or the response could not be read.
    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>, Error> {
        let messages = self.fetch_stream("search", query).await?;

        Ok(parse_search_result_messages(&messages))
    }

    /// Searches Kagi images with the given query and returns the parsed image results.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if a session could not be established, the stream request could not
    /// be sent, or the response could not be read.
    pub async fn images(&self, query: &str) -> Result<Vec<ImageResult>, Error> {
        let messages = self.fetch_stream("images", query).await?;

        Ok(parse_image_result_messages(&messages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_nonce_is_only_taken_once() {
        let mut session = Session {
            nonce: Some("nonce".to_string()),
            created_at: Instant::now(),
        };

        assert_eq!(session.take_nonce().as_deref(), Some("nonce"));
        assert_eq!(session.take_nonce(), None);
    }

    #[test]
    fn test_invalid_language_is_rejected() {
        let options = ClientOptions {
            language: "invalid\nlanguage".to_string(),
            ..ClientOptions::default()
        };

        assert!(matches!(
            Client::with_token_and_options("token", options),
            Err(Error::InvalidHeader(_))
        ));
    }
}
