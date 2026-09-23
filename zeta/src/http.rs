//! HTTP features
//!
//! Two independent clients live here: the plain `reqwest` client shared by the API plugins
//! (behind the `http` feature) and the browser-emulated `wreq` client for anti-bot-protected
//! sites (behind the `emulated` feature).

#[cfg(feature = "http")]
use crate::config::HttpConfig;
#[cfg(feature = "http")]
use crate::error::RequestError;
#[cfg(feature = "http")]
use crate::utils::Truncatable;
#[cfg(feature = "http")]
use serde::de::DeserializeOwned;
#[cfg(feature = "http")]
use tracing::{Span, debug, error, instrument};

#[cfg(feature = "http")]
use crate::url::redact_url;

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
    Deserialize(json::Error),
}

/// The number of characters of an error response body that reaches the logs: enough to read
/// the API's own message, bounded so an HTML error page cannot fill a log line.
#[cfg(feature = "http")]
const LOGGED_BODY_LENGTH: usize = 512;

/// Wraps a failed body read as an [`ApiError`] and records its [`error.type`] on the response
/// span this read happens inside.
///
/// [`error.type`]: https://opentelemetry.io/docs/specs/semconv/registry/attributes/error/
#[cfg(feature = "http")]
fn body_error(error: reqwest::Error) -> ApiError {
    let error = RequestError::from(error);

    Span::current().record("error.type", error.error_type());

    ApiError::Request(error)
}

/// Sends `request` and returns the response.
///
/// A failed send is logged once with `url.full` — the request URL without its query — its
/// `error.type`, and the error's full rendering, before the error comes back as a
/// [`RequestError`]: these APIs carry their key in the query string, and the error reaches
/// `Display` (and `Debug`) in plugin and dispatcher logs, while a log line still needs to say
/// which request failed and why.
///
/// # Errors
///
/// Returns the [`RequestError`] of the failed send, with its URL redacted.
#[cfg(feature = "http")]
pub async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, RequestError> {
    request.send().await.map_err(|error| {
        let error = RequestError::from(error);

        if let Some(url) = error.url() {
            error!(
                url.full = %url,
                error.type = error.error_type(),
                error = %error.full(),
                "request failed"
            );
        }

        error
    })
}

/// Reads the body of `response` as a string.
///
/// A failed read is logged once with `url.full` — the response URL without its query — the
/// way [`send`] logs a failed request, so no caller has to hand-roll that log line.
///
/// # Errors
///
/// Returns the [`RequestError`] of the failed read, with its URL redacted.
#[cfg(feature = "http")]
pub async fn text(response: reqwest::Response) -> Result<String, RequestError> {
    let url = redact_url(response.url());

    response.text().await.map_err(|error| {
        let error = RequestError::from(error);

        error!(
            url.full = %url,
            error.type = error.error_type(),
            error = %error.full(),
            "reading response body failed"
        );

        error
    })
}

/// Parses the response's JSON body into `T`.
///
/// Non-success statuses are reported as [`ApiError::Status`], carrying the response body; the
/// response is only parsed as JSON when the status is a success.
///
/// Everything the OpenTelemetry HTTP conventions define for an outbound request is recorded on
/// the span this opens — <https://opentelemetry.io/docs/specs/semconv/http/http-spans/#http-client-span>
/// — rather than repeated per response: `url.full` (through [`redact_url`], since these APIs
/// carry their key in the query string), `server.address`, `server.port` and
/// `http.response.status_code`, plus `error.type` when something goes wrong.
/// `http.request.method` is missing on purpose — a `reqwest::Response` no longer exposes the
/// request it belongs to — so the span is not a conformant client span; it exists to carry
/// those attributes and the one line that reports the response: a `404` at debug level (it is
/// the routine answer [`parse_response_or_404`] turns into `not_found`), every other
/// non-success status at error level with the truncated body, which is the only place that
/// body reaches the logs.
///
/// # Errors
///
/// Returns an [`ApiError`] if the request fails, the response status is not a success, or the
/// body cannot be parsed as JSON.
#[cfg(feature = "http")]
#[instrument(
    skip(response),
    fields(
        url.full = %redact_url(response.url()),
        server.address = %response.url().host_str().unwrap_or_default(),
        server.port = response.url().port_or_known_default(),
        http.response.status_code = response.status().as_u16(),
        error.type = tracing::field::Empty,
    )
)]
pub async fn parse_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ApiError> {
    let status = response.status();

    if !status.is_success() {
        let not_found = status == reqwest::StatusCode::NOT_FOUND;

        // Report the status even when its body cannot be read: the status is known before the
        // read, and without this line the response would reach the logs only as span
        // attributes. `body_error` records `error.type` first, so the event carries it too.
        let body = response.text().await.map_err(|error| {
            let error = body_error(error);

            if not_found {
                debug!("http response not found");
            } else {
                error!("http response carries an error status");
            }

            error
        })?;
        let logged = body.truncate_within(LOGGED_BODY_LENGTH, "…");

        if not_found {
            debug!(body = %logged, "http response not found");
        } else {
            let error_type = status.as_u16().to_string();
            Span::current().record("error.type", error_type.as_str());
            error!(body = %logged, "http response carries an error status");
        }

        return Err(ApiError::Status { status, body });
    }

    debug!("http response received");

    let text = response.text().await.map_err(body_error)?;

    json::from_str(&text).map_err(|error| {
        Span::current().record("error.type", "invalid_json");

        ApiError::Deserialize(error)
    })
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
#[cfg(all(test, any(feature = "http", feature = "emulated")))]
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
    async fn request_errors_redact_the_url_but_keep_the_cause() {
        let address = refused_address();
        let error = reqwest::Client::new()
            .get(format!("http://{address}/?key=secret"))
            .send()
            .await
            .expect_err("nothing listens on that address");

        // Sanity: reqwest attaches the request URL to the error — redacting it is on us.
        assert!(error.url().is_some(), "reqwest attaches the request url");

        let error = RequestError::from(error);
        let message = error.to_string();
        let full = error.full();

        // The short rendering a channel reply gets: classified, host only — no query, no path,
        // no cause chain, and bounded so it cannot overflow an IRC line.
        assert!(!message.contains("secret"), "{message}");
        assert!(message.starts_with("could not connect"), "{message}");
        assert!(message.contains(&address.ip().to_string()), "{message}");
        assert!(message.len() < 64, "{message}");

        // The full rendering a log line gets: the redacted URL and the cause that reqwest only
        // keeps in `source()`.
        assert!(!full.contains("secret"), "{full}");
        assert!(full.contains(&format!("http://{address}/")), "{full}");
        assert!(full.contains("refused"), "{full}");

        // `Debug` stays free of the query as well...
        assert!(!format!("{error:?}").contains("secret"), "{error:?}");

        // ...and the chain ends here, so nothing that walks `source()` can reach a raw error.
        assert!(std::error::Error::source(&error).is_none(), "{full}");

        let error = ApiError::from(error);

        assert!(!error.to_string().contains("secret"), "{error}");
        assert!(!format!("{error:?}").contains("secret"), "{error:?}");
    }

    #[tokio::test]
    async fn send_logs_a_query_free_url_and_returns_a_redacted_error() {
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

        // The boundary line carries the classified failure, the redacted URL and the cause.
        assert!(logged.contains("error.type"), "{logged}");
        assert!(logged.contains("connection_error"), "{logged}");
        assert!(logged.contains("refused"), "{logged}");

        assert!(!error.to_string().contains("secret"), "{error}");
        assert!(!error.full().contains("secret"), "{error}");
        assert!(!format!("{error:?}").contains("secret"), "{error:?}");
    }

    #[tokio::test]
    async fn parse_response_logs_the_status_even_when_the_body_read_fails() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");

        std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};

            let (mut stream, _) = listener.accept().expect("accept");

            // Drain the request before replying: closing a socket with unread bytes in its
            // receive buffer sends an RST instead of a FIN, which would race with — and
            // sometimes destroy — the response written below.
            let mut request = Vec::new();
            let mut chunk = [0_u8; 512];

            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).expect("read");

                if read == 0 {
                    break;
                }

                request.extend_from_slice(&chunk[..read]);
            }

            // Promise more bytes than are sent, then hang up: the body read fails mid-response.
            let response =
                "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 1024\r\n\r\nshort";
            stream.write_all(response.as_bytes()).expect("write");
        });

        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(buffer.clone()),
        );
        let guard = tracing::subscriber::set_default(subscriber);

        let response = reqwest::Client::new()
            .get(format!("http://{address}/?key=secret"))
            .send()
            .await
            .expect("response");
        let error = parse_response::<serde_json::Value>(response)
            .await
            .expect_err("the body read must fail");
        drop(guard);

        assert!(matches!(error, ApiError::Request(_)), "{error:?}");

        let logged = String::from_utf8(buffer.0.lock().expect("log buffer lock").clone())
            .expect("log is utf-8");

        // The status event fires before the body read gives up...
        assert!(
            logged.contains("http response carries an error status"),
            "{logged}"
        );
        // ...`error.type` is recorded on the span before that event, so it is in the same line...
        assert!(logged.contains("error.type"), "{logged}");
        // ...and no credential from the query reaches the log.
        assert!(!logged.contains("key=secret"), "{logged}");
    }
}

/// Tests for the `wreq` wrapper, in a module of its own: the `emulated`-only builds
/// (`plugin-titles`) reach it without the `http` feature that gates the module above.
#[cfg(all(test, feature = "emulated"))]
mod emulated_tests {
    use super::refused_address;

    #[tokio::test]
    async fn emulated_request_errors_redact_the_uri_but_keep_the_cause() {
        let address = refused_address();
        let error = wreq::Client::builder()
            .build()
            .expect("wreq client")
            .get(format!("http://{address}/?key=secret"))
            .send()
            .await
            .expect_err("nothing listens on that address");

        // Sanity: wreq attaches the request URI to the error — redacting it is on us.
        assert!(error.uri().is_some(), "wreq attaches the request uri");

        let error = crate::error::WreqError::from(error);
        let message = error.to_string();
        let full = error.full();

        // The short rendering a channel reply gets: classified, host only, bounded.
        assert!(!message.contains("secret"), "{message}");
        assert!(message.starts_with("could not connect"), "{message}");
        assert!(message.contains(&address.ip().to_string()), "{message}");
        assert!(message.len() < 64, "{message}");

        // The full rendering a log line gets: the redacted URI and the cause.
        assert!(!full.contains("secret"), "{full}");
        assert!(full.contains(&format!("http://{address}/")), "{full}");
        assert!(full.contains("refused"), "{full}");

        // `wreq` renders its first source itself; walking the chain again would print that
        // link twice.
        assert_eq!(full.matches("client error").count(), 1, "{full}");

        assert!(!format!("{error:?}").contains("secret"), "{error:?}");
        assert!(std::error::Error::source(&error).is_none(), "{full}");
    }
}
