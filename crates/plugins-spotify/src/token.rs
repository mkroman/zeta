//! The OAuth2 client-credentials grant for the Spotify Web API.
//!
//! Spotify authenticates the token request with the application's client id and secret in a
//! Basic authorization header. [`Credentials`] caches the resulting access token and refreshes
//! it shortly before it expires.

use base64::prelude::*;

use zeta_http::oauth::Credentials;
use zeta_http::reqwest;
use zeta_http::reqwest::header::{AUTHORIZATION, CONTENT_TYPE};

/// The Spotify OAuth2 token endpoint.
pub(super) const AUTH_URL: &str = "https://accounts.spotify.com/api/token";

/// Builds the token request for the Spotify client-credentials grant, authenticating the
/// application with a Basic authorization header.
pub(super) fn token_grant(
    client: &reqwest::Client,
    credentials: &Credentials,
) -> reqwest::RequestBuilder {
    let encoded = BASE64_STANDARD.encode(format!(
        "{}:{}",
        credentials.client_id(),
        credentials.client_secret()
    ));

    client
        .post(AUTH_URL)
        .header(AUTHORIZATION, format!("Basic {encoded}"))
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .form(&[("grant_type", "client_credentials")])
}
