#[cfg(feature = "client")]
use std::error::Error as StdError;
#[cfg(feature = "client")]
use std::fmt;
#[cfg(feature = "client")]
use std::fmt::Write as _;

use thiserror::Error;

/// A `reqwest::Error` whose URL has been redacted and whose message is rendered once.
///
/// reqwest prints the request URL in both `Display` and `Debug`, so a wrapped error would put
/// the query string into every log line, span attribute or channel reply that formats it. The
/// URL is stripped from the error — which is dropped — and kept here in its redacted form, so
/// every formatted output is free of it by construction.
///
/// [`Display`](std::fmt::Display) is the short rendering a channel reply needs (`could not
/// connect (ws.dsl.dk)`), while [`RequestError::full`] is what a log line gets: error kind,
/// source chain and redacted URL. [`StdError::source`] reports no source — the chain is already
/// rendered by `full()`, and returning it would print it twice.
#[cfg(feature = "client")]
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

#[cfg(feature = "client")]
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

#[cfg(feature = "client")]
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

#[cfg(feature = "client")]
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

#[cfg(feature = "client")]
impl StdError for RequestError {}

/// Renders the message a log line gets: `kind`, its source chain and its redacted URL.
///
/// A link whose own [`Display`](std::fmt::Display) already ends the message with its source is
/// not appended a second time.
#[cfg(feature = "client")]
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
#[cfg(feature = "client")]
const fn classify(timeout: bool, connect: bool) -> &'static str {
    if timeout {
        "timeout"
    } else if connect {
        "connection_error"
    } else {
        "other"
    }
}

/// Returns `url` as it may be logged: without its query string or userinfo.
#[cfg(feature = "client")]
fn redact_url(url: &reqwest::Url) -> String {
    let mut redacted = url.clone();

    redacted.set_query(None);

    // Only a URL that cannot be a base fails here, and such a URL has no userinfo to strip.
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);

    redacted.to_string()
}

/// The primary error type for this crate.
///
/// This enum consolidates all possible failures, including I/O, HTTP request issues, and HTML
/// parsing errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Occurs when the HTTP client fails to be constructed.
    ///
    /// This error is typically raised by `reqwest` during client initialization and is only
    /// available when the `client` feature is enabled.
    #[cfg(feature = "client")]
    #[error("could not construct http client: {0}")]
    BuildClient(#[source] RequestError),
    /// Represents an error that occurred while sending an HTTP request or reading its body.
    ///
    /// This could be due to a network issue, a timeout, or other `reqwest` internal errors.
    /// This variant is only available when the `client` feature is enabled.
    #[cfg(feature = "client")]
    #[error("request error: {0}")]
    Request(#[source] RequestError),
    /// The server responded with a non-success status code, e.g. `404 Not Found`.
    #[cfg(feature = "client")]
    #[error("the server responded with status {0}")]
    Status(reqwest::StatusCode),
    /// Indicates that a required HTML element could not be found during parsing.
    ///
    /// This error is returned when a CSS selector does not match any element in the document,
    /// preventing the extraction of necessary data. The contained `String` provides context about
    /// what element was being sought.
    #[error("could not find element using selector: {0}")]
    MissingElement(String),
}
