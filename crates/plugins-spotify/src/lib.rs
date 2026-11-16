//! Expands Spotify links and bare URIs into track, album, artist, and playlist details.
//!
//! Links to `open.spotify.com` and `play.spotify.com`, as well as bare `spotify:type:id` URIs
//! pasted as text, are resolved through the Spotify Web API and replied to with the resource
//! name and its artists — plus genres and follower counts for artists, owner and counts for
//! playlists, and the external `spotify:` link only for bare URIs (for link matches it would
//! be redundant). Unknown resource types are ignored; missing resources are noted in the
//! channel.
//!
//! Authentication uses the OAuth2 client-credentials flow: the client id and secret are set in
//! `[plugins.spotify]`, falling back to the `SPOTIFY_CLIENT_ID` and `SPOTIFY_CLIENT_SECRET`
//! environment variables, with the token cached and refreshed by the shared `TokenCache`.
//! Missing credentials fail plugin initialization and the plugin is skipped at startup.
//!
//! The module is split by concern: `commands` fetches a resource and renders its reply, `token`
//! builds the client-credentials token request, and `urls` recognizes links and bare URIs. This
//! module holds the plugin, its settings, and the event routing.

use serde::{Deserialize, Serialize};

use zeta_http as http;
use zeta_http::oauth::Credentials;
use zeta_plugin::{Client, Context, MessageEvent, UrlEvent, ZetaError, plugin, resolve_secret};

mod commands;
mod model;
mod token;
mod urls;

#[cfg(test)]
mod tests;

/// Settings for the spotify plugin, from its `[plugins.spotify]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The Spotify application client id.
    ///
    /// Falls back to the `SPOTIFY_CLIENT_ID` environment variable when unset.
    pub client_id: Option<String>,
    /// The Spotify application client secret.
    ///
    /// Falls back to the `SPOTIFY_CLIENT_SECRET` environment variable when unset.
    pub client_secret: Option<String>,
}

/// Spotify integration plugin.
#[derive(Debug)]
pub struct Spotify {
    credentials: Credentials,
}

/// Errors that can occur while talking to the Spotify API.
pub type Error = http::ApiError;

#[plugin(
    name = "spotify",
    settings = Settings,
    urls(hosts = urls::URL_HOSTS),
    messages
)]
impl Spotify {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client_id = resolve_secret(settings.client_id.as_deref(), "SPOTIFY_CLIENT_ID")?;
        let client_secret =
            resolve_secret(settings.client_secret.as_deref(), "SPOTIFY_CLIENT_SECRET")?;
        let client = ctx.http().clone();
        let credentials = Credentials::new(client, token::AUTH_URL, client_id, client_secret);

        Ok(Self { credentials })
    }

    #[handler(url)]
    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        url: &UrlEvent,
    ) -> Result<(), ZetaError> {
        let channel = url.channel();

        // Handle Spotify URLs (open.spotify.com/type/id); do not include an external URL for
        // link matches, avoiding redundancy.
        if let Some((type_str, id_str)) = urls::parse_spotify_url(url.url()) {
            self.handle_spotify_resource(channel, type_str, id_str, false, client)
                .await?;
        }

        Ok(())
    }

    #[handler(message)]
    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        event: &MessageEvent,
    ) -> Result<(), ZetaError> {
        let channel = event.channel();

        // Handle bare Spotify URIs (spotify:type:id) posted as text.
        for (type_str, id_str) in urls::parse_spotify_uris(event.text()) {
            // Include the external URL for URI matches.
            self.handle_spotify_resource(channel, type_str, id_str, true, client)
                .await?;
        }

        Ok(())
    }
}
