use std::error::Error as StdError;
use std::fmt;
use std::fmt::Write as _;
use std::str::Utf8Error;

use reqwest::header::ToStrError;
use serde_json::Error as JsonError;
use serde_path_to_error::Error as ErrorWithSerdePath;
use url::ParseError;

/// A `reqwest::Error` whose URL has been redacted and whose message is rendered once.
///
/// Reddit carries tokens in query strings, and reqwest prints the request URL in both `Display`
/// and `Debug`, so a wrapped error would put a token into every log line, span attribute or
/// channel reply that formats it. The URL is stripped from the error — which is dropped — and
/// kept here in its redacted form, so every formatted output is free of it by construction.
///
/// [`Display`](std::fmt::Display) is the short rendering a channel reply needs (`could not
/// connect (www.reddit.com)`), while [`RequestError::full`] is what a log line gets: error kind,
/// source chain and redacted URL. [`StdError::source`] reports no source — the chain is already
/// rendered by `full()`, and returning it would print it twice.
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

        match self.host.as_deref() {
            Some(host) => write!(f, " ({host})"),
            None => Ok(()),
        }
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
/// Reddit carries its tokens.
fn redact_url(url: &url::Url) -> String {
    let mut redacted = url.clone();

    redacted.set_query(None);

    // Only a URL that cannot be a base fails here, and such a URL has no userinfo to strip.
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);

    redacted.to_string()
}

/// Errors that can occur while using the Reddit API.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The HTTP client could not be built.
    #[error("could not build http client")]
    BuildClient(#[source] RequestError),
    /// A request could not be sent, or its body could not be read.
    #[error("request error: {0}")]
    Reqwest(#[source] RequestError),
    /// The comments listing could not be parsed.
    #[error("could not deserialize comments json: {0}")]
    DeserializeComments(#[source] ErrorWithSerdePath<JsonError>),
    /// The submission response could not be parsed.
    #[error("could not deserialize submission json: {0}")]
    DeserializeSubmission(#[source] ErrorWithSerdePath<JsonError>),
    /// The subreddit response could not be parsed.
    #[error("could not deserialize subreddit json: {0}")]
    DeserializeSubreddit(#[source] ErrorWithSerdePath<JsonError>),
    /// The subreddit does not exist.
    #[error("subreddit not found")]
    SubredditNotFound,
    /// The submission does not exist.
    #[error("submission not found")]
    SubmissionNotFound,
    /// The server responded with an error status code.
    #[error("http error: {0}")]
    Http(#[source] RequestError),
    /// The response body is in an unexpected format.
    #[error("could not deserialize response as it is in unexpected format")]
    InvalidResponse,
    /// The shortened link did not return a usable redirect url.
    #[error("the shortened link did not return a usable redirect url")]
    InvalidRedirect,
    /// The redirect url uses an invalid encoding.
    #[error("the response redirect url is using an invalid encoding: {0}")]
    RedirectUrlEncoding(#[source] ToStrError),
    /// The short link did not redirect to a submission or comment.
    #[error("expected the short link to redirect to a submission or comment")]
    RedirectRedditLink,
    /// The authentication token request could not be sent.
    #[error("could not request authentication token")]
    RequestAuthToken(#[source] RequestError),
    /// The authentication token response could not be parsed.
    #[error("invalid auth token response")]
    InvalidAuthTokenResponse(#[source] RequestError),
    /// The response did not contain the expected `Location` header.
    #[error("the response did not contain a location header as expected")]
    LocationHeaderMissing,
    /// The response `Location` header contains invalid encoding.
    #[error("the response location header contains invalid encoding")]
    LocationHeaderEncoding(#[source] Utf8Error),
    /// The response `Location` header is not a valid url.
    #[error("the response location header is not a valid url")]
    LocationHeaderUrl(#[source] ParseError),
    /// The video link did not redirect to a reddit submission.
    #[error("video did not redirect to a reddit submission")]
    VideoRedirect,
    /// The video link redirects to something other than a submission.
    #[error("the video link redirects to something other than a submission")]
    VideoRedirectsToNonSubmission,
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::{
        assert_redactions, assert_timeout_redactions, refused_get, timeout_error,
    };

    /// The wrapper's guarantees hold for a real connection failure: no rendering leaks the query
    /// string, the short form is classified and host-only, the full form carries the redacted
    /// URL and the cause, and the chain ends at the wrapper.
    #[tokio::test]
    async fn request_errors_redact_the_url_but_keep_the_cause() {
        let (error, address) = refused_get("token=secret").await;

        let error = RequestError::from(error);
        let message = error.to_string();
        let full = error.full();
        let debug = format!("{error:?}");

        assert_redactions(&message, full, &debug, &address);

        assert_eq!(error.error_type(), "connection_error");

        // The chain ends at the wrapper, so nothing walking `source()` reaches a raw error.
        assert!(std::error::Error::source(&error).is_none(), "{debug}");
    }

    /// The timeout classification: the short form names the timeout, and the query string stays
    /// out of every rendering.
    #[tokio::test]
    async fn timeout_errors_classify_and_redact() {
        let (error, address) = timeout_error().await;

        let error = RequestError::from(error);
        let message = error.to_string();
        let full = error.full();
        let debug = format!("{error:?}");

        assert_timeout_redactions(&message, full, &debug, &address);

        assert_eq!(error.error_type(), "timeout");
    }
}
