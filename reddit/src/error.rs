use std::str::Utf8Error;

use reqwest::header::ToStrError;
use serde_json::Error as JsonError;
use serde_path_to_error::Error as ErrorWithSerdePath;
use url::ParseError;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Reqwest(#[source] reqwest::Error),
    #[error("could not deserialize comments json: {0}")]
    DeserializeComments(#[source] ErrorWithSerdePath<JsonError>),
    #[error("could not deserialize subreddit json: {0}")]
    DeserializeSubreddit(#[source] ErrorWithSerdePath<JsonError>),
    #[error("subreddit not found")]
    SubredditNotFound,
    #[error("submission not found")]
    SubmissionNotFound,
    #[error("http error: {0}")]
    Http(#[source] reqwest::Error),
    #[error("could not deserialize response as it is in unexpected format")]
    InvalidResponse,
    #[error("the shortened link did not return a usable redirect url")]
    InvalidRedirect,
    #[error("the response redirect url is using an invalid encoding: {0}")]
    RedirectUrlEncoding(ToStrError),
    #[error("expected the short link to redirect to a submission or comment")]
    RedirectRedditLink,
    #[error("could not request authentication token")]
    RequestAuthToken(#[source] reqwest::Error),
    #[error("invalid auth token response")]
    InvalidAuthTokenResponse(#[source] reqwest::Error),
    #[error("the response did not contain a location header as expected")]
    LocationHeaderMissing,
    #[error("the response location header contains invalid encoding")]
    LocationHeaderEncoding(#[source] Utf8Error),
    #[error("the response location header is not a valid url")]
    LocationHeaderUrl(#[source] ParseError),
    #[error("video did not redirect to a reddit submission")]
    VideoRedirect,
    #[error("the video link redirects to something other than a submission")]
    VideoRedirectsToNonSubmission,
}
