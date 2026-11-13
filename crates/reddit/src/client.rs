//! Client for the Reddit API.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, LOCATION};
use reqwest::redirect::Policy;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Error as JsonError;
use serde_path_to_error::Error as ErrorWithSerdePath;
use tracing::{Level, debug, instrument, trace};
use url::Url;
use zeta_http::RequestFailure as _;
use zeta_http::json;
use zeta_http::oauth::{TokenCache, TokenResponse};
use zeta_url::RedactedUrl;

use crate::urls::{classify_reddit_com_url, is_id, is_subreddit};
use crate::{
    BASE_URL, Error, HTTP_TIMEOUT, Item, Link, OAUTH_BASE_URL, RequestError, Submission, Subreddit,
    TOKEN_URL, USER_AGENT,
};

/// Reddit client.
// The fields keep Reddit's official OAuth2 `client_id`/`client_secret` terminology, which
// necessarily repeats the struct's name; `http` is the house name for the HTTP client.
#[allow(clippy::struct_field_names)]
pub struct Client {
    /// HTTP client used for requests.
    http: reqwest::Client,
    /// Reddit application client ID.
    client_id: String,
    /// Reddit application client secret.
    client_secret: SecretString,
    /// Current authentication token, refreshed shortly before it expires.
    token: TokenCache,
}

/// Options for building a Reddit [`Client`].
///
/// Unset options keep the crate's defaults: [`USER_AGENT`](crate::USER_AGENT) for the user agent
/// and [`HTTP_TIMEOUT`](crate::HTTP_TIMEOUT) for the timeout.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ClientOptions {
    /// The HTTP user agent sent with every request.
    ///
    /// Defaults to [`USER_AGENT`](crate::USER_AGENT) when unset.
    pub user_agent: Option<String>,
    /// The duration before a HTTP request times out.
    ///
    /// Defaults to [`HTTP_TIMEOUT`](crate::HTTP_TIMEOUT) when unset.
    pub timeout: Option<Duration>,
}

impl Client {
    /// Constructs a new [`Client`] for interacting with the Reddit API using the default
    /// [`ClientOptions`].
    ///
    /// # Examples
    ///
    /// ```
    /// # fn example() -> Result<(), reddit::Error> {
    /// let client_id = "reddit client id";
    /// let client_secret = "reddit client secret";
    /// let client = reddit::Client::try_new(client_id, client_secret)?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::BuildClient`] if the HTTP client fails to build.
    pub fn try_new(
        client_id: impl Into<String>,
        client_secret: impl Into<SecretString>,
    ) -> Result<Self, Error> {
        Self::try_new_with_options(client_id, client_secret, ClientOptions::default())
    }

    /// Constructs a new [`Client`] for interacting with the Reddit API using the given options.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BuildClient`] if the HTTP client fails to build.
    pub fn try_new_with_options(
        client_id: impl Into<String>,
        client_secret: impl Into<SecretString>,
        options: ClientOptions,
    ) -> Result<Self, Error> {
        let client_id = client_id.into();
        let client_secret = client_secret.into();
        let user_agent = options.user_agent.unwrap_or_else(|| USER_AGENT.to_string());
        let http = reqwest::ClientBuilder::new()
            .redirect(Policy::none())
            .timeout(options.timeout.unwrap_or(HTTP_TIMEOUT))
            .user_agent(user_agent)
            .build()
            .map_err(|error| Error::BuildClient(error.into()))?;

        debug!(reddit.client.id = %client_id, "using client id");

        Ok(Self {
            http,
            client_id,
            client_secret,
            token: TokenCache::new(),
        })
    }

    async fn get_valid_token(&self) -> Result<String, Error> {
        self.token.get(|| self.request_access_token()).await
    }

    #[instrument(name = "reddit.auth", skip_all)]
    async fn request_access_token(&self) -> Result<TokenResponse, Error> {
        debug!("access token expired or missing, requesting new token");

        let request = self
            .http
            .post(TOKEN_URL)
            .basic_auth(&self.client_id, Some(self.client_secret.expose_secret()))
            .body("grant_type=client_credentials");
        let response = request.send().await.map_err(|error| {
            Error::RequestAuthToken(RequestError::from(error).logged(Level::DEBUG))
        })?;
        let access_token = response
            .json::<TokenResponse>()
            .await
            .map_err(|error| Error::InvalidAuthTokenResponse(RequestError::from(error)))?;

        Ok(access_token)
    }

    /// Sends the request and parses its JSON body into an [`Item`].
    ///
    /// A `404` status returns `Ok(None)`; any other error status maps to [`Error::Http`].
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, the response is an error status other than `404`,
    /// or the body cannot be parsed.
    async fn send_json(
        &self,
        request: reqwest::RequestBuilder,
        on_parse: fn(ErrorWithSerdePath<JsonError>) -> Error,
    ) -> Result<Option<Item>, Error> {
        let response = request
            .send()
            .await
            .map_err(|error| Error::Reqwest(RequestError::from(error).logged(Level::DEBUG)))?;

        match response.error_for_status() {
            Ok(response) => {
                trace!("response is ok, parsing response");

                let text = response.text().await.map_err(|error| {
                    Error::Reqwest(RequestError::from(error).logged(Level::DEBUG))
                })?;
                let item = json::from_str_with_level(&text, Level::DEBUG).map_err(on_parse)?;

                Ok(Some(item))
            }
            Err(err) if err.status() == Some(StatusCode::NOT_FOUND) => {
                let err = RequestError::from(err);
                debug!(exception.message = %err, "resource not found");

                Ok(None)
            }
            Err(err) => Err(Error::Http(RequestError::from(err).logged(Level::DEBUG))),
        }
    }

    /// Builds an authenticated GET request for `path` on the OAuth API, with the bearer token and
    /// JSON content type set.
    ///
    /// # Errors
    ///
    /// Returns an error if the access token cannot be obtained.
    async fn authorized_get(&self, path: &str) -> Result<reqwest::RequestBuilder, Error> {
        let access_token = self.get_valid_token().await?;

        Ok(self
            .http
            .get(format!("{OAUTH_BASE_URL}{path}"))
            .header(AUTHORIZATION, format!("bearer {access_token}"))
            .header(CONTENT_TYPE, "application/json"))
    }

    /// Fetches and returns details about a given submission.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidId`] if `article` is not a valid Reddit id, and otherwise an error
    /// if the access token cannot be obtained, the request fails, the submission does not exist,
    /// or the response cannot be parsed as a submission.
    #[instrument(name = "reddit.submission", skip_all, fields(reddit.submission.id = %article))]
    pub async fn submission(&self, article: &str) -> Result<Submission, Error> {
        let path = submission_path(article)?;

        debug!("requesting submission");

        let request = self.authorized_get(&path).await?;
        let item = self
            .send_json(request, Error::DeserializeSubmission)
            .await?
            .ok_or(Error::SubmissionNotFound)?;

        match item {
            Item::Listing(listing) => listing
                .children
                .into_iter()
                .find_map(|x| match x {
                    Item::Submission(s) => Some(s),
                    _ => None,
                })
                .ok_or(Error::InvalidResponse),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Fetches and returns details about a given video submission.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidId`] if `id` is not a valid Reddit id,
    /// [`Error::VideoRedirect`] if the video link does not redirect,
    /// [`Error::VideoRedirectsToNonSubmission`] if it redirects to something other than a
    /// submission, or any error from fetching the submission itself.
    #[instrument(name = "reddit.video", skip_all, fields(media.id = %id))]
    pub async fn video(&self, id: &str) -> Result<Submission, Error> {
        let url = video_url(id)?;

        debug!("requesting video redirect location");

        let url = self
            .get_redirect_location(&url)
            .await
            .inspect_err(|error| {
                debug!(
                    exception.message = %error,
                    "the video link did not redirect"
                );
            })
            .map_err(|_| Error::VideoRedirect)?;

        match classify_reddit_com_url(&url) {
            Some(Link::Submission { id, .. }) => self.submission(&id).await,
            _ => Err(Error::VideoRedirectsToNonSubmission),
        }
    }

    /// Fetches and returns details about the subreddit.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidSubreddit`] if `name` is not a valid subreddit name, and otherwise
    /// an error if the access token cannot be obtained, the request fails, the subreddit does
    /// not exist, or the response cannot be parsed as a subreddit.
    #[instrument(name = "reddit.subreddit", skip_all, fields(reddit.subreddit.name = %name))]
    pub async fn subreddit_about_info(&self, name: &str) -> Result<Subreddit, Error> {
        let path = subreddit_about_path(name)?;

        debug!("requesting subreddit details");

        let request = self.authorized_get(&path).await?;
        let item = self
            .send_json(request, Error::DeserializeSubreddit)
            .await?
            .ok_or(Error::SubredditNotFound)?;

        match item {
            Item::Subreddit(subreddit) => Ok(subreddit),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Resolves a shortened subreddit link (`/r/<subreddit>/s/<id>`) to the comment or
    /// submission link it redirects to.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidSubreddit`] if `subreddit` is not a valid subreddit name,
    /// [`Error::InvalidId`] if `id` is not a valid Reddit id, and otherwise an error if the
    /// request fails, the link does not redirect, or the redirect target is not a comment or
    /// submission link.
    pub async fn resolve_shortened_link(&self, subreddit: &str, id: &str) -> Result<Link, Error> {
        let url = shortened_link_url(subreddit, id)?;

        debug!(
            reddit.subreddit.name = %subreddit,
            reddit.shortlink.id = %id,
            "requesting short link to find redirect location"
        );
        let url = self.get_redirect_location(&url).await?;

        match classify_reddit_com_url(&url) {
            Some(x @ (Link::Comment { .. } | Link::Submission { .. })) => Ok(x),
            _ => Err(Error::RedirectRedditLink),
        }
    }

    async fn get_redirect_location(&self, url: &str) -> Result<Url, Error> {
        debug!(
            url.full = %RedactedUrl::from_unparsed(url),
            "fetching redirect location"
        );
        let request = self.http.head(url);
        let response = request
            .send()
            .await
            .map_err(|error| Error::Reqwest(RequestError::from(error).logged(Level::DEBUG)))?;
        let location = response
            .headers()
            .get(LOCATION)
            .ok_or(Error::LocationHeaderMissing)?
            .as_bytes();
        let location = str::from_utf8(location).map_err(Error::LocationHeaderEncoding)?;
        let location = Url::parse(location).map_err(Error::LocationHeaderUrl)?;

        Ok(location)
    }
}

/// Builds the API path for a submission, rejecting an id that is not a valid Reddit id.
///
/// The id is interpolated into an OAuth API path, so validating it before formatting keeps a
/// crafted value from adding path segments, a query string or a fragment to the request.
///
/// # Errors
///
/// Returns [`Error::InvalidId`] when `article` is not a valid Reddit id.
fn submission_path(article: &str) -> Result<String, Error> {
    if !is_id(article) {
        return Err(Error::InvalidId);
    }

    Ok(format!("/by_id/t3_{article}"))
}

/// Builds the URL of a video submission, rejecting an id that is not a valid Reddit id.
///
/// # Errors
///
/// Returns [`Error::InvalidId`] when `id` is not a valid Reddit id.
fn video_url(id: &str) -> Result<String, Error> {
    if !is_id(id) {
        return Err(Error::InvalidId);
    }

    Ok(format!("{BASE_URL}/video/{id}"))
}

/// Builds the API path for a subreddit, rejecting a name that is not a valid subreddit name.
///
/// # Errors
///
/// Returns [`Error::InvalidSubreddit`] when `name` is not a valid subreddit name.
fn subreddit_about_path(name: &str) -> Result<String, Error> {
    if !is_subreddit(name) {
        return Err(Error::InvalidSubreddit);
    }

    Ok(format!("/r/{name}/about"))
}

/// Builds the URL of a shortened subreddit link, rejecting a subreddit name or id that would add
/// segments, a query string or a fragment to the request.
///
/// # Errors
///
/// Returns [`Error::InvalidSubreddit`] when `subreddit` is not a valid subreddit name, and
/// [`Error::InvalidId`] when `id` is not a valid Reddit id.
fn shortened_link_url(subreddit: &str, id: &str) -> Result<String, Error> {
    if !is_subreddit(subreddit) {
        return Err(Error::InvalidSubreddit);
    }

    if !is_id(id) {
        return Err(Error::InvalidId);
    }

    Ok(format!("{BASE_URL}/r/{subreddit}/s/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Segments that would add a path, query or fragment to a request URL, or smuggle one
    /// through percent-encoding, and the empty form that is not an id or name either.
    const CRAFTED_SEGMENTS: &[&str] = &[
        "",
        "..",
        ".",
        "a/b",
        "a?x",
        "a#frag",
        "%2F",
        "%2e%2e",
        "a b",
        "a%20b",
        "a\nb",
        "/by_id/t3_1nh144u",
        "a;b",
    ];

    #[test]
    fn submission_path_accepts_ids_and_rejects_crafted_segments() {
        assert_eq!(submission_path("1nh144u").unwrap(), "/by_id/t3_1nh144u");

        // A fullname is not the bare id the API path expects.
        assert!(matches!(
            submission_path("t3_1nh144u"),
            Err(Error::InvalidId)
        ));

        for segment in CRAFTED_SEGMENTS {
            assert!(
                matches!(submission_path(segment), Err(Error::InvalidId)),
                "{segment:?} should be rejected"
            );
        }
    }

    #[test]
    fn video_url_accepts_ids_and_rejects_crafted_segments() {
        assert_eq!(
            video_url("b2l87x1pyn7h1").unwrap(),
            "https://www.reddit.com/video/b2l87x1pyn7h1"
        );

        assert!(matches!(
            video_url("t3_b2l87x1pyn7h1"),
            Err(Error::InvalidId)
        ));

        for segment in CRAFTED_SEGMENTS {
            assert!(
                matches!(video_url(segment), Err(Error::InvalidId)),
                "{segment:?} should be rejected"
            );
        }
    }

    #[test]
    fn subreddit_about_path_accepts_names_and_rejects_crafted_segments() {
        assert_eq!(
            subreddit_about_path("interestingasfuck").unwrap(),
            "/r/interestingasfuck/about"
        );
        assert_eq!(
            subreddit_about_path("some_sub").unwrap(),
            "/r/some_sub/about"
        );

        for segment in CRAFTED_SEGMENTS {
            assert!(
                matches!(subreddit_about_path(segment), Err(Error::InvalidSubreddit)),
                "{segment:?} should be rejected"
            );
        }
    }

    #[test]
    fn shortened_link_url_accepts_names_and_ids_and_rejects_crafted_segments() {
        assert_eq!(
            shortened_link_url("linuxmemes", "dmwUYLKTjd").unwrap(),
            "https://www.reddit.com/r/linuxmemes/s/dmwUYLKTjd"
        );

        assert!(matches!(
            shortened_link_url("a/b", "dmwUYLKTjd"),
            Err(Error::InvalidSubreddit)
        ));
        assert!(matches!(
            shortened_link_url("linuxmemes", "t3_dmwUYLKTjd"),
            Err(Error::InvalidId)
        ));

        for segment in CRAFTED_SEGMENTS {
            assert!(
                matches!(
                    shortened_link_url("linuxmemes", segment),
                    Err(Error::InvalidId)
                ),
                "{segment:?} should be rejected"
            );
        }
    }

    /// The public methods reject a crafted segment before any token or HTTP work, so the mapped
    /// error reaches the caller instead of a request to an unintended path.
    #[tokio::test]
    async fn methods_reject_crafted_segments_before_the_request() {
        let client = Client::try_new("client id", "client secret").expect("the client builds");

        assert!(matches!(
            client.submission("a/b").await,
            Err(Error::InvalidId)
        ));
        assert!(matches!(client.video("%2F").await, Err(Error::InvalidId)));
        assert!(matches!(
            client.subreddit_about_info("a b").await,
            Err(Error::InvalidSubreddit)
        ));
        assert!(matches!(
            client.resolve_shortened_link("../x", "dmwUYLKTjd").await,
            Err(Error::InvalidSubreddit)
        ));
        assert!(matches!(
            client.resolve_shortened_link("linuxmemes", "a?x").await,
            Err(Error::InvalidId)
        ));
    }
}
