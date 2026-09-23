//! Error types for the Kagi client.

use std::error::Error as StdError;
use std::fmt;
use std::fmt::Write as _;

/// A `reqwest::Error` whose URL has been redacted and whose message is rendered once.
///
/// Kagi puts its login token in the query string, and reqwest prints the request URL in both
/// `Display` and `Debug`, so a wrapped error would put the token into every log line, span
/// attribute or channel reply that formats it. The URL is stripped from the error — which is
/// dropped — and kept here in its redacted form, so every formatted output is free of it by
/// construction.
///
/// [`Display`](std::fmt::Display) is the short rendering a channel reply needs (`could not
/// connect (kagi.com)`), while [`RequestError::full`] is what a log line gets: error kind,
/// source chain and redacted URL. [`StdError::source`] reports no source — the chain is
/// already rendered by `full()`, and returning it would print it twice.
#[derive(Debug)]
pub struct RequestError {
    /// The transport error's own message, with its URL stripped ("error sending request").
    kind: String,
    /// [`kind`] with the source chain and the redacted URL appended — what a log line gets.
    detail: String,
    /// The request URL without its query string or userinfo.
    url: Option<String>,
    /// The host the request went to, for the short rendering.
    host: Option<String>,
    /// Whether the request timed out.
    timeout: bool,
    /// Whether the request failed before it could be sent.
    connect: bool,
}

impl RequestError {
    /// Returns the request URL without its query string or userinfo, if the error carries one.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Returns the full rendering — error kind, source chain and redacted URL — for a log line,
    /// where naming the request that failed matters more than staying short.
    #[must_use]
    pub fn full(&self) -> &str {
        &self.detail
    }

    /// Returns the [`error.type`] value for this failure, for a span or log field.
    ///
    /// [`error.type`]: https://opentelemetry.io/docs/specs/semconv/registry/attributes/error/
    #[must_use]
    pub const fn error_type(&self) -> &'static str {
        classify(self.timeout, self.connect)
    }
}

impl From<reqwest::Error> for RequestError {
    fn from(error: reqwest::Error) -> Self {
        let url = error.url().map(redact_url);
        let host = error
            .url()
            .and_then(|url| url.host_str().map(str::to_string));
        let timeout = error.is_timeout();
        let connect = error.is_connect();

        let error = error.without_url();
        let kind = error.to_string();
        let detail = detail(&kind, &error, url.as_deref());

        Self {
            kind,
            detail,
            url,
            host,
            timeout,
            connect,
        }
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.timeout {
            f.write_str("request timed out")?;
        } else if self.connect {
            f.write_str("could not connect")?;
        } else {
            f.write_str(&self.kind)?;
        }

        self.host
            .as_deref()
            .map_or(Ok(()), |host| write!(f, " ({host})"))
    }
}

impl StdError for RequestError {}

/// Renders the message a log line gets: `kind`, its source chain and its redacted URL.
///
/// A link whose own [`Display`](std::fmt::Display) already ends the message with its source is
/// not appended a second time.
fn detail(kind: &str, error: &(dyn StdError + 'static), url: Option<&str>) -> String {
    let mut message = String::from(kind);
    let mut cause = error.source();

    while let Some(link) = cause {
        let text = link.to_string();

        if !text.is_empty() && !message.ends_with(&text) {
            let _ = write!(message, ": {text}");
        }

        cause = link.source();
    }

    if let Some(url) = url {
        let _ = write!(message, " for url ({url})");
    }

    message
}

/// The low-cardinality [`error.type`] value for a classified transport failure: the attribute
/// OpenTelemetry wants on a failed log line instead of a longer message.
///
/// [`error.type`]: https://opentelemetry.io/docs/specs/semconv/registry/attributes/error/
const fn classify(timeout: bool, connect: bool) -> &'static str {
    if timeout {
        "timeout"
    } else if connect {
        "connection_error"
    } else {
        "other"
    }
}

/// Returns `url` as it may be logged: without its query string or userinfo, which is where
/// Kagi's login token travels.
fn redact_url(url: &reqwest::Url) -> String {
    let mut redacted = url.clone();

    redacted.set_query(None);

    // Only a URL that cannot be a base fails here, and such a URL has no userinfo to strip.
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);

    redacted.to_string()
}

/// Errors that can occur while searching with Kagi.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The stream request could not be sent.
    #[error("could not send stream request")]
    StreamRequest(#[source] RequestError),
    /// The stream request returned an error HTTP status.
    #[error("stream request returned an error status")]
    StreamStatus(#[source] RequestError),
    /// The response body of the stream request could not be read.
    #[error("could not read response body of stream request")]
    StreamRequestBody(#[source] RequestError),
    /// The nonce request could not be sent.
    #[error("could not send nonce request")]
    RequestNonce(#[source] RequestError),
    /// The nonce response could not be read.
    #[error("could not read nonce response")]
    ReadNonce(#[source] RequestError),
    /// The session request could not be sent.
    #[error("could not send session request")]
    RequestSession(#[source] RequestError),
    /// The response did not include session cookies, indicating an invalid login token.
    #[error("response did not include session valid cookies - is the login token valid?")]
    SessionCookies,
    /// The response did not include a nonce.
    #[error("response did not include a nonce")]
    Nonce,
    /// A configured header value is invalid.
    #[error("invalid header value: {0}")]
    InvalidHeader(#[from] reqwest::header::InvalidHeaderValue),
    /// The HTTP client could not be built.
    #[error("could not build http client")]
    BuildClient(#[source] RequestError),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wrapper's guarantees hold for a real transport failure: no rendering leaks the query
    /// string, the short form is classified and host-only, the full form carries the redacted
    /// URL and the cause, and the chain ends at the wrapper.
    #[tokio::test]
    async fn request_errors_redact_the_url_but_keep_the_cause() {
        // Bind-then-drop, so the connection below is refused deterministically.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let address = listener.local_addr().expect("listener address");
        drop(listener);

        let error = reqwest::Client::new()
            .get(format!("http://{address}/?login_token=secret"))
            .send()
            .await
            .expect_err("nothing listens on that address");

        // Sanity: reqwest attaches the request URL to the error — redacting it is on us.
        assert!(error.url().is_some(), "reqwest attaches the request url");

        let error = RequestError::from(error);
        let message = error.to_string();
        let full = error.full();
        let debug = format!("{error:?}");

        // The login token in the query string must not reach any rendering.
        assert!(!message.contains("secret"), "{message}");
        assert!(!full.contains("secret"), "{full}");
        assert!(!debug.contains("secret"), "{debug}");

        // The short rendering is classified, names only the host and stays bounded.
        assert!(message.starts_with("could not connect"), "{message}");
        assert!(message.contains(&address.ip().to_string()), "{message}");
        assert!(message.len() < 64, "{message}");

        // The full rendering carries the redacted URL and the cause chain.
        assert!(full.contains(&format!("http://{address}/")), "{full}");
        assert!(full.contains("refused"), "{full}");
        assert_eq!(error.error_type(), "connection_error");

        // The chain ends at the wrapper, so nothing walking `source()` reaches a raw error.
        assert!(std::error::Error::source(&error).is_none(), "{debug}");
    }
}
