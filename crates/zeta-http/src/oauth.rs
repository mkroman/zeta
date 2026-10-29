//! OAuth2 client-credentials token caching shared by the API client plugins.
//!
//! Plugins that talk to an OAuth2-secured API (Twitch, Spotify, ..) all need the same
//! machinery: request an access token with the client-credentials grant, cache it, and refresh
//! it shortly before it expires. [`TokenCache`] implements that on top of
//! [`zeta_cache::TtlCache`], parameterized over the plugin-specific token request itself, and
//! [`Credentials`] pairs it with the application's credentials and the shared HTTP client.

use std::future::Future;
use std::time::Duration;

use serde::Deserialize;
use tracing::debug;
use zeta_cache::TtlCache;

/// How long before its actual expiry a cached token is considered stale.
///
/// The buffer keeps in-flight requests from using a token that expires mid-request.
const EXPIRY_BUFFER: Duration = Duration::from_mins(1);

/// The response of an OAuth2 token endpoint for the client-credentials grant.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    /// The access token issued by the authorization server.
    pub access_token: String,
    /// The lifetime of the access token, in seconds.
    pub expires_in: u64,
}

/// A single-slot cache of an OAuth2 client-credentials access token.
///
/// The token is cached for its `expires_in` lifetime minus the expiry buffer, and concurrent
/// misses request one token. Cloning is not supported; share the cache by reference from the
/// owner.
pub struct TokenCache {
    /// The cached access token, stale once its lifetime minus the expiry buffer has passed.
    token: TtlCache<String>,
}

impl TokenCache {
    /// Creates an empty cache.
    #[must_use]
    pub const fn new() -> Self {
        // The cache-wide TTL is unused: every token carries its own lifetime, which `get`
        // supplies through `TtlCache::get_or_refresh_with`.
        Self {
            token: TtlCache::new(Duration::ZERO),
        }
    }

    /// Returns a valid access token, requesting a new one through `refresh` when the cached
    /// token is missing or within `EXPIRY_BUFFER` of expiring.
    ///
    /// `refresh` should perform the client-credentials grant request and return the token
    /// endpoint's response; the token is cached for its `expires_in` lifetime minus the expiry
    /// buffer. Concurrent misses request one token: the refresh is single-flight, and the tasks
    /// that wait for it pick up the token the first refresh produced.
    ///
    /// # Errors
    ///
    /// Returns the error produced by `refresh` if it fails to obtain a new token.
    pub async fn get<E, F, R>(&self, refresh: R) -> Result<String, E>
    where
        R: FnOnce() -> F + Send,
        F: Future<Output = Result<TokenResponse, E>> + Send,
    {
        self.token
            .get_or_refresh_with(|| async move {
                let response = refresh().await?;
                let lifetime = Duration::from_secs(response.expires_in);

                Ok((
                    response.access_token,
                    lifetime.saturating_sub(EXPIRY_BUFFER),
                ))
            })
            .await
    }
}

impl Default for TokenCache {
    fn default() -> Self {
        Self::new()
    }
}

/// An OAuth2 client-credentials API client, shared by the plugins that authenticate through
/// token endpoints (Spotify, Twitch).
///
/// Holds the HTTP client shared by the token request and the API's own requests, the
/// application credentials, and the cached access token — refreshed shortly before it expires
/// through `TokenCache`.
pub struct Credentials {
    /// The HTTP client used for the token request and the API's own requests.
    client: reqwest::Client,
    /// The token endpoint the client-credentials grant is sent to.
    auth_url: String,
    /// The application client id.
    client_id: String,
    /// The application client secret.
    client_secret: String,
    /// The cached access token.
    token: TokenCache,
}

impl Credentials {
    /// Creates a client for the OAuth2 token endpoint `auth_url`.
    #[must_use]
    pub fn new(
        client: reqwest::Client,
        auth_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        Self {
            client,
            auth_url: auth_url.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            token: TokenCache::new(),
        }
    }

    /// Returns the shared HTTP client, for the API's own requests.
    #[must_use]
    pub const fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Returns the application client id, needed by some APIs' own requests.
    #[must_use]
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Returns the application client secret, needed to authenticate the grant.
    #[must_use]
    pub fn client_secret(&self) -> &str {
        &self.client_secret
    }

    /// Returns a valid access token, requesting a new one through the client-credentials grant
    /// when the cached token is missing or within the expiry buffer of expiring.
    ///
    /// `grant` builds the token request against the shared HTTP client, applying the API's
    /// client authentication scheme (a Basic authorization header for Spotify, form-encoded
    /// credentials for Twitch).
    ///
    /// # Errors
    ///
    /// Returns an `http::ApiError` when the token request fails or its response cannot be
    /// parsed.
    pub async fn access_token(
        &self,
        grant: impl FnOnce(&reqwest::Client, &Self) -> reqwest::RequestBuilder + Send,
    ) -> Result<String, crate::ApiError> {
        self.token
            .get(|| async {
                debug!(auth_url = %self.auth_url, "refreshing oauth2 access token");
                let token = crate::get_json(grant(&self.client, self)).await?;

                Ok(token)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn cached_token(cache: &TokenCache) -> Option<String> {
        cache.token.get()
    }

    #[tokio::test]
    async fn refreshes_the_token_only_once_while_fresh() {
        let requests = Arc::new(AtomicUsize::new(0));
        let cache = TokenCache::new();

        let refresh = || {
            let requests = Arc::clone(&requests);

            async move {
                let access_token = format!("token-{}", requests.load(Ordering::SeqCst));
                requests.fetch_add(1, Ordering::SeqCst);

                Ok::<_, ()>(TokenResponse {
                    access_token,
                    expires_in: 3600,
                })
            }
        };

        let first = cache.get(refresh).await.unwrap();
        let second = cache.get(refresh).await.unwrap();

        assert_eq!(first, "token-0");
        assert_eq!(second, first);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(cached_token(&cache).as_deref(), Some("token-0"));
    }

    #[tokio::test]
    async fn tokens_that_expire_within_the_buffer_are_not_cached() {
        let cache = TokenCache::new();
        let requests = Arc::new(AtomicUsize::new(0));

        // A token whose lifetime is shorter than the expiry buffer is stale as soon as it is
        // cached, so every read requests a new one.
        for _ in 0..2 {
            let requests = Arc::clone(&requests);

            let token = cache
                .get(|| async move {
                    requests.fetch_add(1, Ordering::SeqCst);

                    Ok::<_, ()>(TokenResponse {
                        access_token: "short-lived".to_owned(),
                        expires_in: 30,
                    })
                })
                .await
                .unwrap();

            assert_eq!(token, "short-lived");
        }

        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn surfaces_refresh_errors_without_caching() {
        let cache = TokenCache::new();

        let refresh = || async { Err("denied") };

        assert_eq!(cache.get(refresh).await, Err("denied"));
        assert_eq!(cached_token(&cache), None);
    }

    #[tokio::test]
    async fn concurrent_misses_request_one_token() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let cache = Arc::new(TokenCache::new());
        let requests = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();

        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let requests = Arc::clone(&requests);

            handles.push(tokio::spawn(async move {
                cache
                    .get(|| async {
                        requests.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;

                        Ok::<_, ()>(TokenResponse {
                            access_token: "token".to_owned(),
                            expires_in: 3600,
                        })
                    })
                    .await
                    .unwrap()
            }));
        }

        for handle in handles {
            assert_eq!(handle.await.unwrap(), "token");
        }

        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn access_token_requests_one_token_through_the_grant() {
        use serde_json::json;
        use zeta_test_support::wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};

        let server = MockServer::start().await;

        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "token",
                "expires_in": 3600,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let uri = server.uri();
        let credentials = Credentials::new(
            reqwest::Client::new(),
            "https://auth.example/token",
            "client-id",
            "client-secret",
        );
        let grant = |client: &reqwest::Client, _: &Credentials| {
            client
                .post(&uri)
                .form(&[("grant_type", "client_credentials")])
        };

        // The second call is served from the cache, so the token endpoint is hit once.
        let first = credentials.access_token(grant).await.unwrap();
        let second = credentials.access_token(grant).await.unwrap();

        assert_eq!(first, "token");
        assert_eq!(second, "token");
    }
}
