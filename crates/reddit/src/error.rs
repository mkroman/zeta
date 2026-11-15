//! Error types for the Reddit client.

use std::str::Utf8Error;

use serde_json::Error as JsonError;
use serde_path_to_error::Error as ErrorWithSerdePath;
use url::ParseError;

/// Re-export of the shared transport error, which strips the token-bearing request URL
/// before anything formats it.
pub use zeta_http::RequestError;

/// Errors that can occur while using the Reddit API.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// The HTTP client could not be built.
    #[error("could not build http client")]
    BuildClient(#[source] RequestError),
    /// A request could not be sent, or its body could not be read.
    #[error("request error: {0}")]
    Reqwest(#[source] RequestError),
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
    /// The submission, comment, or video id is not a valid Reddit id.
    #[error("invalid reddit id")]
    InvalidId,
    /// The subreddit name is not a valid Reddit subreddit name.
    #[error("invalid subreddit name")]
    InvalidSubreddit,
    /// The server responded with an error status code.
    #[error("http error: {0}")]
    Http(#[source] RequestError),
    /// The response body is in an unexpected format.
    #[error("could not deserialize response as it is in unexpected format")]
    InvalidResponse,
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
