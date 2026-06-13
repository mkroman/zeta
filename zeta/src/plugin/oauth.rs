//! Shared OAuth2 Client Credentials Flow token caching for API plugins.

// Parts of this API are unused when only a subset of the consuming plugins is enabled.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::debug;

/// How long before expiry a cached token is considered stale.
const EXPIRY_BUFFER: Duration = Duration::from_mins(1);

/// How the client credentials are sent to the token endpoint.
#[derive(Clone, Copy, Debug)]
pub enum AuthStyle {
    /// HTTP Basic authentication header (e.g. Spotify).
    BasicHeader,
    /// `client_id`/`client_secret` fields in the form body (e.g. Twitch).
    FormBody,
}

/// OAuth2 Client Credentials Flow client with token caching.
///
/// Holds the application credentials and a cached access token, refreshing it
/// when it is within [`EXPIRY_BUFFER`] of expiring.
pub struct ClientCredentials {
    /// OAuth2 token endpoint URL.
    auth_url: &'static str,
    /// Application client ID.
    client_id: String,
    /// Application client secret.
    client_secret: String,
    /// How credentials are sent to the token endpoint.
    style: AuthStyle,
    /// Cached access token.
    token: RwLock<Option<Token>>,
}

/// A cached OAuth2 access token.
#[derive(Clone, Debug)]
struct Token {
    /// The access token string.
    access_token: String,
    /// The time at which the token expires.
    expires_at: Instant,
}

/// Response from an OAuth2 token endpoint.
#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
    expires_in: u64,
}

impl ClientCredentials {
    /// Creates a new credentials holder with an empty token cache.
    pub const fn new(
        auth_url: &'static str,
        client_id: String,
        client_secret: String,
        style: AuthStyle,
    ) -> Self {
        Self {
            auth_url,
            client_id,
            client_secret,
            style,
            token: RwLock::const_new(None),
        }
    }

    /// Returns the application client ID.
    #[must_use]
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Returns a valid access token, refreshing it if necessary.
    pub async fn access_token(&self, client: &reqwest::Client) -> Result<String, reqwest::Error> {
        if let Some(token) = self.token.read().await.as_ref()
            && token.expires_at > Instant::now() + EXPIRY_BUFFER
        {
            return Ok(token.access_token.clone());
        }

        debug!(auth_url = self.auth_url, "refreshing oauth2 access token");
        let request = client.post(self.auth_url);
        let request = match self.style {
            AuthStyle::BasicHeader => request
                .basic_auth(&self.client_id, Some(&self.client_secret))
                .form(&[("grant_type", "client_credentials")]),
            AuthStyle::FormBody => request.form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("grant_type", "client_credentials"),
            ]),
        };

        let auth: AuthResponse = request.send().await?.error_for_status()?.json().await?;
        let token = Token {
            access_token: auth.access_token.clone(),
            expires_at: Instant::now() + Duration::from_secs(auth.expires_in),
        };

        *self.token.write().await = Some(token);

        Ok(auth.access_token)
    }
}
