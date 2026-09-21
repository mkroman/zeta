//! Expands Twitch links with stream, clip, and video details.
//!
//! Links to `twitch.tv`, `www.twitch.tv`, and `clips.twitch.tv` are resolved through the
//! Twitch Helix API: a channel URL shows the live stream's title, game, and viewer count (or a
//! plain `<channel> - Twitch` line when offline), a clip URL shows its title, broadcaster,
//! creator, and view count, and a `videos/<id>` URL shows the video's title, user, and view
//! count. URLs that match none of the resource kinds — including channel subpaths — are
//! ignored.
//!
//! Authentication uses the OAuth2 client-credentials flow: the application client id and
//! secret are set in `[plugins.twitch]`, falling back to the `TWITCH_CLIENT_ID` and
//! `TWITCH_CLIENT_SECRET` environment variables, with the access token cached and refreshed by
//! the shared `TokenCache`. Missing credentials fail plugin initialization and the plugin is
//! skipped at startup.

use num_format::{Locale, ToFormattedString};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{http, oauth::Credentials, plugin::prelude::*};

mod urls;

use self::urls::{UrlKind, parse_twitch_url};

/// Twitch OAuth2 token endpoint.
const AUTH_URL: &str = "https://id.twitch.tv/oauth2/token";
/// Twitch Helix API base URL.
const BASE_URL: &str = "https://api.twitch.tv/helix";

/// Settings for the twitch plugin, from its `[plugins.twitch]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Settings {
    /// The Twitch application client id.
    ///
    /// Falls back to the `TWITCH_CLIENT_ID` environment variable when unset.
    #[serde(default)]
    pub client_id: Option<String>,
    /// The Twitch application client secret.
    ///
    /// Falls back to the `TWITCH_CLIENT_SECRET` environment variable when unset.
    #[serde(default)]
    pub client_secret: Option<String>,
}

/// Twitch.tv integration plugin.
///
/// This plugin listens for Twitch.tv URLs in messages and expands them with
/// information about the stream, clip, or video.
pub struct Twitch {
    /// The client-credentials API client.
    credentials: Credentials,
}

/// Errors that can occur during Twitch plugin execution.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Twitch API returned an error response.
    #[error(transparent)]
    Api(#[from] http::ApiError),
    /// An irc error occurred while sending the reply.
    #[error("irc error: {0}")]
    Irc(#[from] irc::error::Error),
}

/// Generic response wrapper for Twitch Helix API endpoints.
#[derive(Deserialize, Debug)]
struct Response<T> {
    data: Vec<T>,
}

/// Represents a Twitch stream.
#[derive(Deserialize, Debug)]
struct Stream {
    user_login: String,
    #[allow(dead_code)]
    user_name: String,
    game_name: String,
    title: String,
    viewer_count: u64,
}

/// Represents a Twitch clip.
#[derive(Deserialize, Debug)]
struct Clip {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    url: String,
    title: String,
    creator_name: String,
    broadcaster_name: String,
    view_count: u64,
}

/// Represents a Twitch video.
#[derive(Deserialize, Debug)]
struct Video {
    #[allow(dead_code)]
    id: String,
    title: String,
    user_name: String,
    view_count: u64,
}

#[async_trait]
impl Plugin<Context> for Twitch {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.urls(UrlScope::Hosts(urls::URL_HOSTS));

        let client_id = resolve_secret(settings.client_id.as_deref(), "TWITCH_CLIENT_ID")?;
        let client_secret =
            resolve_secret(settings.client_secret.as_deref(), "TWITCH_CLIENT_SECRET")?;
        let client = http::build_client(&ctx.config.http);
        let credentials = Credentials::new(client, AUTH_URL, client_id, client_secret);

        Ok(Self { credentials })
    }

    async fn handle_url(&self, _ctx: &Context, client: &Client, url: &UrlEvent) -> Result<(), ZetaError> {
        let channel = url.channel();

        if let Some(kind) = parse_twitch_url(url.url()) {
            let result = match kind {
                UrlKind::Stream(login) => self.handle_stream(channel, &login, client).await,
                UrlKind::Clip(id) => self.handle_clip(channel, &id, client).await,
                UrlKind::Video(id) => self.handle_video(channel, &id, client).await,
            };

            if let Err(e) = result {
                warn!("Twitch plugin error: {}", e);
            }
        }

        Ok(())
    }
}

impl Twitch {
    /// Builds the token request for the Twitch client-credentials grant, authenticating the
    /// application with form-encoded credentials.
    fn token_grant(client: &reqwest::Client, credentials: &Credentials) -> reqwest::RequestBuilder {
        let params = [
            ("client_id", credentials.client_id()),
            ("client_secret", credentials.client_secret()),
            ("grant_type", "client_credentials"),
        ];

        client.post(AUTH_URL).form(&params)
    }

    /// Helper to make authenticated GET requests to the Helix API.
    async fn get<T: for<'de> Deserialize<'de>>(
        &self,
        endpoint: &str,
        query: &[(&str, &str)],
    ) -> Result<Response<T>, Error> {
        let token = self
            .credentials
            .access_token(Twitch::token_grant)
            .await
            .map_err(Error::from)?;
        let url = format!("{BASE_URL}/{endpoint}");

        let response = self
            .credentials
            .client()
            .get(&url)
            .header("Client-ID", self.credentials.client_id())
            .header("Authorization", format!("Bearer {token}"))
            .query(query)
            .send()
            .await
            .map_err(http::ApiError::Request)?;

        http::parse_response(response).await.map_err(Error::from)
    }

    /// Fetches stream information and sends a message to the channel.
    async fn handle_stream(
        &self,
        channel: &str,
        user_login: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let response: Response<Stream> = self.get("streams", &[("user_login", user_login)]).await?;

        if let Some(stream) = response.data.first() {
            let user_login = &stream.user_login;
            let title = &stream.title;
            let game_name = &stream.game_name;
            let viewers = stream.viewer_count.to_formatted_string(&Locale::en);

            client.send_privmsg(channel, reply("Twitch", format!(
                "{user_login}:\x0f {title}\x0310 - Game:\x0f {game_name}\x0310 Viewers:\x0f {viewers}\x0310"
            )))?;
        } else {
            // Fallback behavior: just print the channel name if not live.
            client.send_privmsg(channel, notice(format!("{user_login} - Twitch")))?;
        }

        Ok(())
    }

    /// Fetches clip information and sends a message to the channel.
    async fn handle_clip(
        &self,
        channel: &str,
        clip_id: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let response: Response<Clip> = self.get("clips", &[("id", clip_id)]).await?;

        if let Some(clip) = response.data.first() {
            let title = &clip.title;
            let broadcaster = &clip.broadcaster_name;
            let creator = &clip.creator_name;
            let views = clip.view_count.to_formatted_string(&Locale::en);

            client.send_privmsg(channel, reply("Twitch", format!(
                "“\x0f{title}\x0310” is a clip of\x0f {broadcaster}\x0310 clipped by\x0f {creator}\x0310 with\x0f {views}\x0310 views"
            )))?;
        } else {
            client.send_privmsg(channel, reply("Twitch", "No results"))?;
        }

        Ok(())
    }

    /// Fetches video information and sends a message to the channel.
    async fn handle_video(
        &self,
        channel: &str,
        video_id: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let response: Response<Video> = self.get("videos", &[("id", video_id)]).await?;

        if let Some(video) = response.data.first() {
            let title = &video.title;
            let user = &video.user_name;
            let views = video.view_count.to_formatted_string(&Locale::en);

            client.send_privmsg(channel, reply("Twitch", format!(
                "“\x0f{title}\x0310” is a video by\x0f {user}\x0310 with\x0f {views}\x0310 views"
            )))?;
        } else {
            client.send_privmsg(channel, reply("Twitch", "No results"))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.client_id.is_none());
            assert!(settings.client_secret.is_none());
        }
        deserialize: {
            "client_id": "id",
            "client_secret": "secret",
        } assert: {
            assert_eq!(settings.client_id.as_deref(), Some("id"));
            assert_eq!(settings.client_secret.as_deref(), Some("secret"));
        }
    }
}
