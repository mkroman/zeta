//! HTTP features
//!
//! Two independent clients live here: the plain `reqwest` client shared by the API plugins
//! (behind the `http` feature) and the browser-emulated `wreq` client for anti-bot-protected
//! sites (behind the `emulated` feature).

#[cfg(feature = "http")]
use crate::config::HttpConfig;
#[cfg(feature = "http")]
use serde::de::DeserializeOwned;
#[cfg(feature = "http")]
use tracing::{debug, error, instrument};

/// JSON response parsing shared by the API client plugins.
///
/// Bodies are parsed with [`serde_path_to_error`] so parse failures report the path to the
/// offending part of the document, and the offending body is logged.
#[cfg(feature = "http")]
pub mod json {
    use serde::de::DeserializeOwned;
    use tracing::error;

    /// The error returned when a JSON body fails to parse.
    pub type Error = serde_path_to_error::Error<serde_json::Error>;

    /// Deserializes a value of type `T` from a JSON string.
    ///
    /// The offending body is logged if parsing fails.
    ///
    /// # Errors
    ///
    /// Returns the parse error, including the path to the offending part of the document, if
    /// `text` is not valid JSON for `T`.
    pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
        let deserializer = &mut serde_json::Deserializer::from_str(text);

        serde_path_to_error::deserialize(deserializer).inspect_err(|error| {
            error!(?error, body = %text, "could not deserialize json response");
        })
    }
}

/// The errors produced when sending a request and parsing its JSON response.
#[cfg(feature = "http")]
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The request could not be sent, or the response body could not be read.
    ///
    /// Built through [`From<reqwest::Error>`], which strips the URL: these APIs carry their key
    /// in the query string, and the error reaches `Display` (and `Debug`) in plugin and
    /// dispatcher logs.
    #[error("request error: {0}")]
    Request(#[source] reqwest::Error),
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
    Deserialize(json::Error),
}

impl From<reqwest::Error> for ApiError {
    /// Strips the URL from `error` before wrapping it, so no logged error can carry a query
    /// string with a credential in it.
    fn from(error: reqwest::Error) -> Self {
        Self::Request(error.without_url())
    }
}

/// Sends `request` and returns the response.
///
/// A failed send is logged once with `url.full` — the request URL without its query — before
/// the URL is stripped from the returned [`reqwest::Error`]: these APIs carry their key in
/// the query string, and the error reaches `Display` (and `Debug`) in plugin and dispatcher
/// logs, while a log line still needs to say which request failed.
///
/// # Errors
///
/// Returns the [`reqwest::Error`] of the failed send, with its URL stripped.
#[cfg(feature = "http")]
pub async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, reqwest::Error> {
    request.send().await.map_err(|error| {
        let url = error.url().map(|url| {
            let mut url = url.clone();
            url.set_query(None);
            url
        });
        let error = error.without_url();

        if let Some(url) = url {
            error!(url.full = %url, %error, "request failed");
        }

        error
    })
}

/// Sends a request built by [`reqwest::Client::get`] (or a sibling builder method) and parses
/// its JSON body into `T`.
///
/// Non-success statuses are reported as [`ApiError::Status`], carrying the response body; the
/// response is only parsed as JSON when the status is a success.
///
/// Every response is logged once, with the attributes the OpenTelemetry HTTP conventions
/// define for an outbound request: <https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-client-span>.
/// `http.request.method` is missing on purpose — a `reqwest::Response` no longer exposes the
/// request it belongs to — and the logged URL carries no query: these APIs carry their key in
/// the query string. A `404` is logged at debug
/// level (it is the routine answer [`parse_response_or_404`] turns into `not_found`); every
/// other non-success status is logged as an error.
///
/// # Errors
///
/// Returns an [`ApiError`] if the request fails, the response status is not a success, or the
/// body cannot be parsed as JSON.
#[cfg(feature = "http")]
#[instrument(
    skip(response),
    fields(url.full = %({
        let mut url = response.url().clone();
        url.set_query(None);
        url
    }))
)]
pub async fn parse_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ApiError> {
    let status = response.status();
    let url = {
        let mut url = response.url().clone();
        url.set_query(None);
        url
    };
    let server_address = response.url().host_str().unwrap_or_default().to_owned();
    let server_port = response.url().port_or_known_default();

    if !status.is_success() {
        if status == reqwest::StatusCode::NOT_FOUND {
            debug!(
                url.full = %url,
                server.address = %server_address,
                server.port = server_port,
                http.response.status_code = status.as_u16(),
                "http response not found",
            );
        } else {
            error!(
                url.full = %url,
                server.address = %server_address,
                server.port = server_port,
                http.response.status_code = status.as_u16(),
                "http response carries an error status",
            );
        }

        let body = response.text().await.map_err(ApiError::from)?;

        return Err(ApiError::Status { status, body });
    }

    debug!(
        url.full = %url,
        server.address = %server_address,
        server.port = server_port,
        http.response.status_code = status.as_u16(),
        "http response received",
    );

    let text = response.text().await.map_err(ApiError::from)?;

    json::from_str(&text).map_err(ApiError::Deserialize)
}

/// Parses a JSON response like [`parse_response`], mapping a `404` status to `not_found`.
///
/// The remaining [`ApiError`]s are converted into `E`.
///
/// # Errors
///
/// Returns `not_found` if the response status is `404 Not Found`; any other [`parse_response`]
/// error is converted into `E`.
#[cfg(feature = "http")]
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

/// HTTP client integration
#[cfg(feature = "http")]
pub mod client {
    use crate::config::HttpConfig;

    pub use reqwest::Client;
    use reqwest::redirect::Policy;

    /// Returns a default HTTP client configured by [`HttpConfig`].
    ///
    /// # Panics
    ///
    /// Panics if the default HTTP client fails to build.
    #[must_use]
    pub fn build(config: &HttpConfig) -> Client {
        builder(config)
            .build()
            .expect("could not build http client")
    }

    /// Returns a default HTTP client builder configured by [`HttpConfig`].
    pub fn builder(config: &HttpConfig) -> reqwest::ClientBuilder {
        reqwest::ClientBuilder::new()
            .redirect(Policy::none())
            .timeout(config.timeout)
            .user_agent(config.user_agent.clone())
    }
}

/// Builds a default HTTP client configured by [`HttpConfig`].
///
/// This is equivalent to calling [`client::build`].
#[must_use]
#[cfg(feature = "http")]
pub fn build_client(config: &HttpConfig) -> client::Client {
    client::build(config)
}

/// Client building for anti-bot-protected sites.
///
/// These are fetched with [`wreq`] browser emulation: plain `reqwest` gets TLS-fingerprinted by
/// them regardless of headers. Built behind the `emulated` feature, which the anti-bot-protected
/// plugins enable.
#[cfg(feature = "emulated")]
pub mod emulated {
    use wreq::header::{ACCEPT_ENCODING, HeaderMap, HeaderValue, USER_AGENT};
    use zeta_plugin::{Error, prelude::plugin_err};

    /// Returns a `wreq` client builder emulating Firefox 142, layered with the headers the
    /// emulation must not omit.
    ///
    /// `Accept-Encoding` must be set explicitly: like reqwest, wreq does not advertise the
    /// header itself even though it decompresses responses — and its absence is enough to get
    /// flagged.
    ///
    /// The profile's user agent is overridden with `user_agent` when given. This skews with the
    /// emulated Firefox 142 fingerprint, and that is deliberate: the profile defaults to macOS —
    /// which anti-bot systems score far more aggressively when requests originate from
    /// datacenter networks — and the matching Linux Firefox 142 user agent is rejected by
    /// DataDome outright, while Firefox 151 passes.
    ///
    /// # Errors
    ///
    /// Returns an error if `user_agent` cannot be used as a header value.
    pub fn builder(user_agent: Option<&str>) -> Result<wreq::ClientBuilder, Error> {
        let mut headers = HeaderMap::new();

        headers.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br, zstd"),
        );

        if let Some(user_agent) = user_agent {
            let value = HeaderValue::from_str(user_agent).map_err(plugin_err)?;

            headers.insert(USER_AGENT, value);
        }

        Ok(wreq::Client::builder()
            .emulation(wreq_util::Emulation::Firefox142)
            .default_headers(headers))
    }
}

/// Returns the address of a just-dropped local listener, so connections to it are refused
/// deterministically while URLs built against them still carry credential-looking queries.
#[cfg(all(test, feature = "http"))]
pub fn refused_address() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    drop(listener);
    address
}

#[cfg(all(test, feature = "http"))]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::prelude::*;

    use super::*;

    /// A writer that collects everything logged through it, for assertions.
    #[derive(Clone, Default)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
        type Writer = LogBuffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn request_errors_never_carry_the_url() {
        let error = reqwest::Client::new()
            .get(format!("http://{}/?key=secret", refused_address()))
            .send()
            .await
            .expect_err("nothing listens on that address");

        // Sanity: reqwest attaches the request URL to the error — stripping it is on us.
        assert!(error.url().is_some(), "reqwest attaches the request url");

        let error = ApiError::from(error);

        assert!(!error.to_string().contains("secret"), "{error}");
        assert!(!format!("{error:?}").contains("secret"), "{error:?}");
    }

    #[tokio::test]
    async fn send_logs_a_query_free_url_and_returns_a_stripped_error() {
        let address = refused_address();
        let request_url = format!("http://{address}/?key=secret");

        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(buffer.clone()),
        );
        let guard = tracing::subscriber::set_default(subscriber);

        let error = send(reqwest::Client::new().get(&request_url))
            .await
            .expect_err("nothing listens on that address");
        drop(guard);

        let logged = String::from_utf8(buffer.0.lock().expect("log buffer lock").clone())
            .expect("log is utf-8");

        let query_free = format!("http://{address}/");
        assert!(logged.contains("url.full"), "{logged}");
        assert!(logged.contains(&query_free), "{logged}");
        assert!(!logged.contains("key=secret"), "{logged}");

        assert!(!error.to_string().contains("secret"), "{error}");
        assert!(!format!("{error:?}").contains("secret"), "{error:?}");
    }
}
