use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, LOCATION};
use reqwest::redirect::Policy;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::Error as JsonError;
use serde_path_to_error::Error as ErrorWithSerdePath;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, trace};
use url::Url;
use zeta_url::RedactedUrl;

use crate::{BASE_URL, HTTP_TIMEOUT, LOGGED_BODY_LENGTH, OAUTH_BASE_URL, TOKEN_URL, USER_AGENT};
use crate::{Error, Item, Link, RequestError, Submission, Subreddit};

/// Wraps `error` in a [`RequestError`] — whose URL is redacted — and logs the failed request's
/// URL without its query, so a logged error never carries a credential from the query string.
fn request_error(error: reqwest::Error) -> RequestError {
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
}

struct TokenCache {
    access_token: String,
    expires_at: Instant,
}

#[derive(Deserialize)]
struct AccessTokenResponse {
    pub access_token: String,
    pub expires_in: u64,
}

/// Reddit client.
// The fields keep Reddit's official OAuth2 `client_id`/`client_secret` terminology, which
// necessarily repeats the struct's name; `client` is the house name for the HTTP client.
#[allow(clippy::struct_field_names)]
pub struct Client {
    /// HTTP client used for requests.
    client: reqwest::Client,
    /// Reddit application client ID.
    client_id: String,
    /// Reddit application client secret.
    client_secret: SecretString,
    /// Current authentication token state.
    token_state: RwLock<Option<TokenCache>>,
}

impl TokenCache {
    pub fn is_valid(&self) -> bool {
        self.expires_at > Instant::now() + Duration::from_secs(30)
    }
}

/// Options for building a Reddit [`Client`].
#[derive(Clone, Debug, Default)]
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
    /// let client_id = "reddit client id";
    /// let client_secret = "reddit client secret";
    /// let client = reddit::Client::new(client_id, client_secret);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client fails to build.
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<SecretString>) -> Self {
        Self::with_options(client_id, client_secret, ClientOptions::default())
            .expect("could not build http client")
    }

    /// Constructs a new [`Client`] for interacting with the Reddit API using the given options.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BuildClient`] if the HTTP client fails to build.
    pub fn with_options(
        client_id: impl Into<String>,
        client_secret: impl Into<SecretString>,
        options: ClientOptions,
    ) -> Result<Self, Error> {
        let client_id = client_id.into();
        let client_secret = client_secret.into();
        let token_state = RwLock::new(None);
        let user_agent = options.user_agent.unwrap_or_else(|| USER_AGENT.to_string());
        let client = reqwest::ClientBuilder::new()
            .redirect(Policy::none())
            .timeout(options.timeout.unwrap_or(HTTP_TIMEOUT))
            .user_agent(user_agent)
            .build()
            .map_err(|error| Error::BuildClient(error.into()))?;

        debug!("using client id {client_id}");

        Ok(Self {
            client,
            client_id,
            client_secret,
            token_state,
        })
    }

    // The write guard is deliberately held across the token request — that is what
    // serializes concurrent misses into a single token request.
    #[allow(clippy::significant_drop_tightening)]
    async fn get_valid_token(&self) -> Result<String, Error> {
        {
            let read = self.token_state.read().await;

            if let Some(cache) = read.as_ref().filter(|state| state.is_valid()) {
                return Ok(cache.access_token.clone());
            }
        }

        let new_token = {
            let mut guard = self.token_state.write().await;

            // Double check and return the current session if it was changed while we were waiting
            // for the lock.
            if let Some(cache) = guard.as_ref().filter(|state| state.is_valid()) {
                return Ok(cache.access_token.clone());
            }

            debug!("access token expired or missing, requesting new token");
            let auth_token = self.request_access_token().await?;

            *guard = Some(TokenCache {
                access_token: auth_token.access_token.clone(),
                expires_at: Instant::now() + Duration::from_secs(auth_token.expires_in),
            });

            auth_token.access_token
        };

        Ok(new_token)
    }

    #[instrument(skip(self))]
    async fn request_access_token(&self) -> Result<AccessTokenResponse, Error> {
        let request = self
            .client
            .post(TOKEN_URL)
            .basic_auth(&self.client_id, Some(self.client_secret.expose_secret()))
            .body("grant_type=client_credentials");
        let response = request
            .send()
            .await
            .map_err(|error| Error::RequestAuthToken(request_error(error)))?;
        let access_token = response
            .json::<AccessTokenResponse>()
            .await
            .map_err(|error| {
                let error = RequestError::from(error);
                error!("auth token response is invalid: {error}");
                Error::InvalidAuthTokenResponse(error)
            })?;

        Ok(access_token)
    }

    /// Sends the request and parses its JSON body into an [`Item`].
    ///
    /// A `404` status maps to `not_found`, any other error status to [`Error::Http`].
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, the response is an error status, or the body
    /// cannot be parsed.
    async fn send_json(
        &self,
        request: reqwest::RequestBuilder,
        not_found: fn() -> Error,
        on_parse: fn(ErrorWithSerdePath<JsonError>) -> Error,
    ) -> Result<Item, Error> {
        let response = request
            .send()
            .await
            .map_err(|error| Error::Reqwest(request_error(error)))?;

        match response.error_for_status() {
            Ok(response) => {
                trace!("response is ok, parsing response");

                let text = response
                    .text()
                    .await
                    .map_err(|error| Error::Reqwest(request_error(error)))?;
                let deserializer = &mut serde_json::Deserializer::from_str(&text);

                serde_path_to_error::deserialize(deserializer)
                    .inspect_err(|err| {
                        let body = text.chars().take(LOGGED_BODY_LENGTH).collect::<String>();

                        error!(?err, body = %body, "could not parse response");
                    })
                    .map_err(on_parse)
            }
            Err(err) if err.status() == Some(StatusCode::NOT_FOUND) => {
                let err = RequestError::from(err);
                info!(%err, "resource not found");

                Err(not_found())
            }
            Err(err) => Err(Error::Http(request_error(err))),
        }
    }

    /// Fetches and returns details about a given submission.
    ///
    /// # Errors
    ///
    /// Returns an error if the access token cannot be obtained, the request fails, the
    /// submission does not exist, or the response cannot be parsed as a submission.
    #[instrument(skip(self))]
    pub async fn submission(&self, article: &str) -> Result<Submission, Error> {
        let access_token = self.get_valid_token().await?;
        debug!(%article, "requesting submission");

        let request = self
            .client
            .get(format!("{OAUTH_BASE_URL}/by_id/t3_{article}"))
            .header(AUTHORIZATION, format!("bearer {access_token}"))
            .header(CONTENT_TYPE, "application/json");

        match self
            .send_json(
                request,
                || Error::SubmissionNotFound,
                Error::DeserializeSubmission,
            )
            .await?
        {
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
    /// Returns [`Error::VideoRedirect`] if the video link does not redirect,
    /// [`Error::VideoRedirectsToNonSubmission`] if it redirects to something other than a
    /// submission, or any error from fetching the submission itself.
    #[instrument(skip(self))]
    pub async fn video(&self, id: &str) -> Result<Submission, Error> {
        debug!(%id, "requesting video redirect location");

        let url = self
            .get_redirect_location(&format!("{BASE_URL}/video/{id}"))
            .await
            .inspect_err(|error| debug!(%id, %error, "the video link did not redirect"))
            .map_err(|_| Error::VideoRedirect)?;

        match crate::classify_reddit_com_url(&url) {
            Some(Link::Submission { id, .. }) => self.submission(&id).await,
            _ => Err(Error::VideoRedirectsToNonSubmission),
        }
    }

    /// Fetches and returns details about the subreddit.
    ///
    /// # Errors
    ///
    /// Returns an error if the access token cannot be obtained, the request fails, the
    /// subreddit does not exist, or the response cannot be parsed as a subreddit.
    #[instrument(skip(self))]
    pub async fn subreddit_about_info(&self, name: &str) -> Result<Subreddit, Error> {
        let access_token = self.get_valid_token().await?;
        debug!(%name, "requesting subreddit details");

        let request = self
            .client
            .get(format!("{OAUTH_BASE_URL}/r/{name}/about"))
            .header(AUTHORIZATION, format!("bearer {access_token}"))
            .header(CONTENT_TYPE, "application/json");

        match self
            .send_json(
                request,
                || Error::SubredditNotFound,
                Error::DeserializeSubreddit,
            )
            .await?
        {
            Item::Subreddit(subreddit) => Ok(subreddit),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Resolves a shortened subreddit link (`/r/<subreddit>/s/<id>`) to the comment or
    /// submission link it redirects to.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, the link does not redirect, or the redirect
    /// target is not a comment or submission link.
    pub async fn resolve_shortened_link(&self, subreddit: &str, id: &str) -> Result<Link, Error> {
        debug!(%subreddit, %id, "requesting short link to find redirect location");
        let url = self
            .get_redirect_location(&format!("{BASE_URL}/r/{subreddit}/s/{id}"))
            .await?;

        match crate::classify_reddit_com_url(&url) {
            Some(x @ (Link::Comment { .. } | Link::Submission { .. })) => Ok(x),
            _ => Err(Error::RedirectRedditLink),
        }
    }

    async fn get_redirect_location(&self, url: &str) -> Result<Url, Error> {
        debug!(
            url.full = %RedactedUrl::from_unparsed(url),
            "fetching redirect location"
        );
        let request = self.client.head(url);
        let response = request
            .send()
            .await
            .map_err(|error| Error::Reqwest(request_error(error)))?;
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
