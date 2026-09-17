//! OAuth2 client-credentials token caching shared by the API client plugins.
//!
//! Plugins that talk to an OAuth2-secured API (Twitch, Spotify, ..) all need the same
//! machinery: request an access token with the client-credentials grant, cache it, and refresh
//! it shortly before it expires. [`TokenCache`] implements that, parameterized over the
//! plugin-specific token request itself.

use std::future::Future;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::RwLock;

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
/// Cloning is not supported; share the cache by reference from the owner.
pub struct TokenCache {
    token: RwLock<Option<CachedToken>>,
}

/// A cached token with its absolute expiry time.
struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

impl TokenCache {
    /// Creates an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: RwLock::new(None),
        }
    }

    /// Returns a valid access token, requesting a new one through `refresh` when the cached
    /// token is missing or within [`EXPIRY_BUFFER`] of expiring.
    ///
    /// `refresh` should perform the client-credentials grant request and return the token
    /// endpoint's response.
    ///
    /// # Errors
    ///
    /// Returns the error produced by `refresh` if it fails to obtain a new token.
    pub async fn get<E, F, R>(&self, refresh: R) -> Result<String, E>
    where
        R: FnOnce() -> F,
        F: Future<Output = Result<TokenResponse, E>>,
    {
        if let Some(token) = self.token.read().await.as_ref()
            && token.expires_at > Instant::now() + EXPIRY_BUFFER
        {
            return Ok(token.access_token.clone());
        }

        let response = refresh().await?;
        let expires_at = Instant::now() + Duration::from_secs(response.expires_in);

        *self.token.write().await = Some(CachedToken {
            access_token: response.access_token.clone(),
            expires_at,
        });

        Ok(response.access_token)
    }
}

impl Default for TokenCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    async fn cached_token(cache: &TokenCache) -> Option<String> {
        cache
            .token
            .read()
            .await
            .as_ref()
            .map(|token| token.access_token.clone())
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
        assert_eq!(cached_token(&cache).await.as_deref(), Some("token-0"));
    }

    #[tokio::test]
    async fn surfaces_refresh_errors_without_caching() {
        let cache = TokenCache::new();

        let refresh = || async { Err("denied") };

        assert_eq!(cache.get(refresh).await, Err("denied"));
        assert_eq!(cached_token(&cache).await, None);
    }
}
