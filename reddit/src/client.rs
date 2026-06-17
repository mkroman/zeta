use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, LOCATION};
use reqwest::{StatusCode, redirect::Policy};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, trace};
use url::Url;

use crate::{BASE_URL, HTTP_TIMEOUT, OAUTH_BASE_URL, USER_AGENT};
use crate::{Error, Item, Link, Submission, Subreddit};

struct TokenCache {
    access_token: String,
    expires_at: Instant,
}

#[derive(Clone, Eq, PartialEq, Deserialize)]
struct AccessTokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub scope: String,
}

/// Reddit client.
pub struct Client {
    /// HTTP client used for requests.
    client: reqwest::Client,
    /// Reddit application client ID.
    client_id: String,
    /// Reddit application client secret.
    client_secret: SecretString,
    /// Current authentication token state.
    token_state: Arc<RwLock<Option<TokenCache>>>,
}

impl TokenCache {
    pub fn is_valid(&self) -> bool {
        self.expires_at > Instant::now() + Duration::from_secs(30)
    }
}

impl Client {
    /// Constructs a new [`Client`] for interacting with the Reddit API.
    ///
    /// # Examples
    ///
    /// ```
    /// let client_id = "reddit client id";
    /// let client_secret = "reddit client secret";
    /// let client = reddit::Client::new(client_id, client_secret, None);
    /// ```
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<SecretString>,
        user_agent: Option<String>,
    ) -> Client {
        let client_id = client_id.into();
        let client_secret = client_secret.into();
        let token_state = Arc::new(RwLock::new(None));
        let user_agent = user_agent.unwrap_or_else(|| USER_AGENT.to_string());
        let client = reqwest::ClientBuilder::new()
            .redirect(Policy::none())
            .timeout(HTTP_TIMEOUT)
            .user_agent(user_agent.clone())
            .build()
            .expect("could not build http client");

        debug!("using client id {client_id}");

        Client {
            client,
            client_id,
            client_secret,
            token_state,
        }
    }

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
            .post("https://www.reddit.com/api/v1/access_token")
            .basic_auth(&self.client_id, Some(self.client_secret.expose_secret()))
            .body("grant_type=client_credentials");
        let response = request.send().await.map_err(Error::RequestAuthToken)?;
        let access_token = response
            .json::<AccessTokenResponse>()
            .await
            .inspect_err(|e| error!("auth token response is invalid: {e}"))
            .map_err(Error::InvalidAuthTokenResponse)?;

        Ok(access_token)
    }

    /// Fetches and returns details about a given submission.
    #[instrument(skip(self))]
    pub async fn submission(&self, article: &str) -> Result<Submission, Error> {
        let access_token = self.get_valid_token().await?;
        debug!("requesting submission");

        let request = self
            .client
            .get(format!("{OAUTH_BASE_URL}/by_id/t3_{article}"))
            .header(AUTHORIZATION, format!("bearer {access_token}"))
            .header(CONTENT_TYPE, "application/json");
        let response = request.send().await.map_err(Error::Reqwest)?;

        match response.error_for_status() {
            Ok(response) => {
                trace!("response is ok, parsing list");

                let text = response.text().await.map_err(Error::Reqwest)?;
                let jd = &mut serde_json::Deserializer::from_str(&text);
                let listing: Item = serde_path_to_error::deserialize(jd)
                    .inspect_err(|err| error!(?err, %text, "could not parse list"))
                    .map_err(Error::DeserializeComments)?;
                trace!(x = ?(&listing), "finished parsing list");

                match listing {
                    Item::Listing(listing) => listing
                        .children
                        .into_iter()
                        .find_map(|x| match x {
                            Item::Submission(s) => Some(s),
                            _ => None,
                        })
                        .ok_or_else(|| Error::InvalidResponse),
                    _ => Err(Error::InvalidResponse),
                }
            }
            Err(err) if err.status() == Some(StatusCode::NOT_FOUND) => {
                info!(%article, %err, "could not fetch details for article");

                Err(Error::SubmissionNotFound)
            }
            Err(err) => Err(Error::Http(err)),
        }
    }

    /// Fetches and returns details about a given video submission.
    #[instrument(skip(self))]
    pub async fn video(&self, id: &str) -> Result<Submission, Error> {
        debug!("requesting video redirect location");
        let Ok(url) = self
            .get_redirect_location(&format!("{BASE_URL}/video/{id}"))
            .await
        else {
            return Err(Error::VideoRedirect);
        };

        match crate::classify_reddit_com_url(&url) {
            Some(Link::Submission { id, .. }) => self.submission(&id).await,
            _ => Err(Error::VideoRedirectsToNonSubmission),
        }
    }

    /// Fetches and returns details about the subreddit.
    #[instrument(skip(self))]
    pub async fn subreddit_about_info(&self, name: &str) -> Result<Subreddit, Error> {
        let access_token = self.get_valid_token().await?;
        debug!("requesting submission");

        let request = self
            .client
            .get(format!("{OAUTH_BASE_URL}/r/{name}/about"))
            .header(AUTHORIZATION, format!("bearer {access_token}"))
            .header(CONTENT_TYPE, "application/json");

        debug!("requesting subreddit details");
        let response = request.send().await.map_err(Error::Reqwest)?;

        match response.error_for_status() {
            Ok(response) => {
                trace!("response is ok, parsing subreddit");

                let text = response.text().await.map_err(Error::Reqwest)?;
                let jd = &mut serde_json::Deserializer::from_str(&text);
                let item: Item = serde_path_to_error::deserialize(jd)
                    .inspect_err(|err| error!(?err, %text, "could not parse subreddit response"))
                    .map_err(Error::DeserializeSubreddit)?;
                debug!(?item, "finished parsing item");

                match item {
                    Item::Subreddit(subreddit) => Ok(subreddit),
                    _ => Err(Error::InvalidResponse),
                }
            }
            Err(err) if err.status() == Some(StatusCode::NOT_FOUND) => {
                info!(%name, %err, "subreddit not found");

                Err(Error::SubredditNotFound)
            }
            Err(err) => Err(Error::Http(err)),
        }
    }

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
        debug!("fetching redirect location for url {url}");
        let request = self.client.head(url);
        let response = request.send().await.map_err(Error::Reqwest)?;
        let location = response
            .headers()
            .get(LOCATION)
            .ok_or_else(|| Error::LocationHeaderMissing)?
            .as_bytes();
        let location = str::from_utf8(location).map_err(Error::LocationHeaderEncoding)?;
        let location = Url::parse(location).map_err(Error::LocationHeaderUrl)?;

        Ok(location)
    }
}
