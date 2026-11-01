//! Error types for the CoinMarketCap client.

use std::error::Error as StdError;
use std::fmt;
use std::fmt::Write as _;

use zeta_url::RedactedUrl;

/// A `reqwest::Error` whose URL has been redacted and whose message is rendered once.
///
/// The CoinMarketCap API key travels in a request header rather than the URL, but the URL still
/// reaches logs, span attributes and channel replies when an error is formatted, and reqwest
/// prints the request URL in both `Display` and `Debug`. The URL is stripped from the error —
/// which is dropped — and kept here in its redacted form, so every formatted output is free of
/// the query string by construction.
///
/// [`Display`](std::fmt::Display) is the short rendering a channel reply needs (`could not
/// connect (pro-api.coinmarketcap.com)`), while [`RequestError::full`] is what a log line gets:
/// error kind, source chain and redacted URL. [`StdError::source`] reports no source — the chain
/// is already rendered by `full()`, and returning it would print it twice.
#[derive(Debug)]
pub struct RequestError {
    /// The transport error's own message, with its URL stripped ("error sending request").
    kind: String,
    /// [`kind`] with the source chain and the redacted URL appended — what a log line gets.
    detail: String,
    /// The request URL without its query string, fragment, or userinfo.
    url: Option<RedactedUrl>,
    /// The host the request went to, for the short rendering.
    host: Option<String>,
    /// Whether the request timed out.
    timeout: bool,
    /// Whether the request failed before it could be sent.
    connect: bool,
}

impl RequestError {
    /// Returns the request URL without its query string, fragment, or userinfo, if the error
    /// carries one.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        self.url.as_ref().map(RedactedUrl::as_str)
    }

    /// Returns the full rendering — error kind, source chain and redacted URL — for a log
    /// line, where naming the request that failed matters more than staying short.
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
        let url = error.url().map(RedactedUrl::from_url);
        let host = error
            .url()
            .and_then(|url| url.host_str().map(str::to_string));
        let timeout = error.is_timeout();
        let connect = error.is_connect();

        let error = error.without_url();
        let kind = error.to_string();
        let detail = detail(&kind, &error, url.as_ref().map(RedactedUrl::as_str));

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
        write_short(
            f,
            &self.kind,
            self.host.as_deref(),
            self.timeout,
            self.connect,
        )
    }
}

impl StdError for RequestError {}

/// Renders the message a log line gets: `kind`, its source chain and its redacted URL.
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

/// Writes the short rendering a channel reply needs: the classified failure and the host it
/// happened to, never the path (which a user can make arbitrarily long) or the cause chain.
fn write_short(
    f: &mut fmt::Formatter<'_>,
    kind: &str,
    host: Option<&str>,
    timeout: bool,
    connect: bool,
) -> fmt::Result {
    if timeout {
        f.write_str("request timed out")?;
    } else if connect {
        f.write_str("could not connect")?;
    } else {
        f.write_str(kind)?;
    }

    host.map_or(Ok(()), |host| write!(f, " ({host})"))
}

/// The low-cardinality [`error.type`] value for a classified transport failure: the
/// attribute OpenTelemetry wants on a failed client span instead of a longer message.
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

/// The JSON response body could not be parsed.
pub type ParseError = serde_json::Error;

/// Errors that can occur during coin lookups.
///
/// The [`Display`](std::fmt::Display) representation of each variant is the message that is
/// replied to the channel.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The client could not be built.
    #[error("Could not reach the CoinMarketCap API")]
    Build(#[source] RequestError),
    /// The configured API key is not a valid HTTP header value.
    #[error("Invalid CoinMarketCap API key")]
    InvalidApiKey(#[source] reqwest::header::InvalidHeaderValue),
    /// The HTTP request failed.
    #[error("Could not reach the CoinMarketCap API")]
    Request(#[from] RequestError),
    /// The response could not be parsed.
    #[error("Could not parse the response from the CoinMarketCap API")]
    Deserialize(#[source] ParseError),
    /// The API returned an error status (e.g. an invalid symbol).
    #[error("Could not retrieve coin information: {0}")]
    Api(String),
    /// An unsupported conversion currency was requested.
    #[error("Unsupported currency: {0}")]
    InvalidCurrency(String),
    /// The requested coin was not found.
    #[error("No such coin")]
    NotFound,
}
