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

use zeta_http as http;
use zeta_http::oauth::Credentials;
use zeta_plugin::prelude::*;
use zeta_utils::irc::unstyle_text;

mod urls;

use self::urls::{UrlKind, parse_twitch_url};

/// Twitch OAuth2 token endpoint.
const AUTH_URL: &str = "https://id.twitch.tv/oauth2/token";
/// Twitch Helix API base URL.
const BASE_URL: &str = "https://api.twitch.tv/helix";

/// Settings for the twitch plugin, from its `[plugins.twitch]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The Twitch application client id.
    ///
    /// Falls back to the `TWITCH_CLIENT_ID` environment variable when unset.
    pub client_id: Option<String>,
    /// The Twitch application client secret.
    ///
    /// Falls back to the `TWITCH_CLIENT_SECRET` environment variable when unset.
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

#[plugin(name = "twitch", settings = Settings, urls(hosts = urls::URL_HOSTS))]
impl Twitch {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client_id = resolve_secret(settings.client_id.as_deref(), "TWITCH_CLIENT_ID")?;
        let client_secret =
            resolve_secret(settings.client_secret.as_deref(), "TWITCH_CLIENT_SECRET")?;
        let client = http::build_client(&ctx.config.http);
        let credentials = Credentials::new(client, AUTH_URL, client_id, client_secret);

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
            .access_token(Self::token_grant)
            .await
            .map_err(Error::from)?;
        let url = format!("{BASE_URL}/{endpoint}");

        let request = self
            .credentials
            .client()
            .get(&url)
            .header("Client-ID", self.credentials.client_id())
            .header("Authorization", format!("Bearer {token}"))
            .query(query);
        http::get_json(request).await.map_err(Error::from)
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
            let user_login = unstyle_text(&stream.user_login);
            let title = unstyle_text(&stream.title);
            let game_name = unstyle_text(&stream.game_name);
            let viewers = stream.viewer_count.to_formatted_string(&Locale::en);

            client.send_privmsg(channel, labeled_reply("Twitch", format!(
                "{user_login}:\x0f {title}\x0310 - Game:\x0f {game_name}\x0310 Viewers:\x0f {viewers}\x0310"
            )))?;
        } else {
            // Fallback behavior: just print the channel name if not live.
            client.send_privmsg(channel, plain_reply(format!("{user_login} - Twitch")))?;
        }

        Ok(())
    }

    /// Fetches the first result of an endpoint and replies with its formatted description.
    async fn handle_first<T: for<'de> Deserialize<'de>>(
        &self,
        endpoint: &str,
        query: &[(&str, &str)],
        channel: &str,
        client: &Client,
        render: impl FnOnce(&T) -> String,
    ) -> Result<(), Error> {
        let response: Response<T> = self.get(endpoint, query).await?;

        let message = response.data.first().map_or_else(
            || labeled_reply("Twitch", "No results"),
            |item| labeled_reply("Twitch", render(item)),
        );

        client.send_privmsg(channel, message)?;

        Ok(())
    }

    /// Fetches clip information and sends a message to the channel.
    async fn handle_clip(
        &self,
        channel: &str,
        clip_id: &str,
        client: &Client,
    ) -> Result<(), Error> {
        self.handle_first("clips", &[("id", clip_id)], channel, client, |clip: &Clip| {
            let title = unstyle_text(&clip.title);
            let broadcaster = unstyle_text(&clip.broadcaster_name);
            let creator = unstyle_text(&clip.creator_name);
            let views = clip.view_count.to_formatted_string(&Locale::en);

            format!(
                "“\x0f{title}\x0310” is a clip of\x0f {broadcaster}\x0310 clipped by\x0f {creator}\x0310 with\x0f {views}\x0310 views"
            )
        })
        .await
    }

    /// Fetches video information and sends a message to the channel.
    async fn handle_video(
        &self,
        channel: &str,
        video_id: &str,
        client: &Client,
    ) -> Result<(), Error> {
        self.handle_first("videos", &[("id", video_id)], channel, client, |video: &Video| {
            let title = &video.title;
            let user = &video.user_name;
            let views = video.view_count.to_formatted_string(&Locale::en);

            format!(
                "“\x0f{title}\x0310” is a video by\x0f {user}\x0310 with\x0f {views}\x0310 views"
            )
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

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
