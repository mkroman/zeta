use std::str::Utf8Error;

use reqwest::header::ToStrError;
use serde_json::Error as JsonError;
use serde_path_to_error::Error as ErrorWithSerdePath;
use url::ParseError;

/// Errors that can occur while using the Reddit API.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// A request could not be sent, or its body could not be read.
    #[error("request error: {0}")]
    Reqwest(#[source] reqwest::Error),
    /// The comments listing could not be parsed.
    #[error("could not deserialize comments json: {0}")]
    DeserializeComments(#[source] ErrorWithSerdePath<JsonError>),
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
    Http(#[source] reqwest::Error),
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
    RequestAuthToken(#[source] reqwest::Error),
    /// The authentication token response could not be parsed.
    #[error("invalid auth token response")]
    InvalidAuthTokenResponse(#[source] reqwest::Error),
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
