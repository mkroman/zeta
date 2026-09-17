#![allow(clippy::doc_markdown)]

use num_format::{Locale, ToFormattedString};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use url::Url;

use crate::{
    http,
    oauth::{TokenCache, TokenResponse},
    plugin::prelude::*,
};

/// Twitch OAuth2 token endpoint.
const AUTH_URL: &str = "https://id.twitch.tv/oauth2/token";
/// Twitch Helix API base URL.
const BASE_URL: &str = "https://api.twitch.tv/helix";

/// Settings for the twitch plugin, from its `[plugins.twitch]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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
    /// HTTP client used for requests.
    client: reqwest::Client,
    /// Twitch application client ID.
    client_id: String,
    /// Twitch application client secret.
    client_secret: String,
    /// Cached OAuth2 access token.
    token: TokenCache,
}

/// Errors that can occur during Twitch plugin execution.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("api error: {0}")]
    Api(String),
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

/// The type of Twitch resource found in a URL.
#[derive(Debug)]
enum UrlKind {
    Stream(String),
    Clip(String),
    Video(String),
}

#[async_trait]
impl Plugin<Context> for Twitch {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client_id = resolve_secret(settings.client_id.as_deref(), "TWITCH_CLIENT_ID")?;
        let client_secret =
            resolve_secret(settings.client_secret.as_deref(), "TWITCH_CLIENT_SECRET")?;
        let client = http::build_client(&ctx.config.http);

        Ok(Self {
            client,
            client_id,
            client_secret,
            token: TokenCache::new(),
        })
    }

    fn url_hosts(&self) -> &'static [&'static str] {
        &["clips.twitch.tv", "twitch.tv", "www.twitch.tv"]
    }

    async fn handle_message(
        &self,
        ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        let Command::PRIVMSG(channel, _) = &message.command else {
            return Ok(());
        };

        for url in FilteredUrls::from_message(ctx, message)
            .into_iter()
            .flatten()
        {
            if let Some(kind) = Self::parse_url(&url) {
                let result = match kind {
                    UrlKind::Stream(login) => self.handle_stream(channel, &login, client).await,
                    UrlKind::Clip(id) => self.handle_clip(channel, &id, client).await,
                    UrlKind::Video(id) => self.handle_video(channel, &id, client).await,
                };

                if let Err(e) = result {
                    warn!("Twitch plugin error: {}", e);
                }
            }
        }

        Ok(())
    }
}

impl Twitch {
    /// Authenticates with Twitch using Client Credentials Flow.
    ///
    /// Returns a valid access token, refreshing it if necessary.
    async fn get_token(&self) -> Result<String, Error> {
        self.token
            .get(|| async {
                debug!("refreshing twitch access token");
                let params = [
                    ("client_id", self.client_id.as_str()),
                    ("client_secret", self.client_secret.as_str()),
                    ("grant_type", "client_credentials"),
                ];

                let response = self.client.post(AUTH_URL).form(&params).send().await?;
                let auth: TokenResponse = response.error_for_status()?.json().await?;

                Ok(auth)
            })
            .await
    }

    /// Helper to make authenticated GET requests to the Helix API.
    async fn get<T: for<'de> Deserialize<'de>>(
        &self,
        endpoint: &str,
        query: &[(&str, &str)],
    ) -> Result<Response<T>, Error> {
        let token = self.get_token().await?;
        let url = format!("{BASE_URL}/{endpoint}");

        let response = self
            .client
            .get(&url)
            .header("Client-ID", &self.client_id)
            .header("Authorization", format!("Bearer {token}"))
            .query(query)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(Error::Api(format!("status: {}", response.status())));
        }

        Ok(response.json().await?)
    }

    /// Parses a Twitch URL and determines the resource type.
    fn parse_url(url: &Url) -> Option<UrlKind> {
        let host = url.host_str()?;
        let segments: Vec<&str> = url.path_segments()?.collect();

        if host == "twitch.tv" || host == "www.twitch.tv" {
            match segments.as_slice() {
                // twitch.tv/videos/<id>
                ["videos", id] if !id.is_empty() => Some(UrlKind::Video(id.to_string())),
                // twitch.tv/<channel>/clip/<id>
                [_, "clip", id] if !id.is_empty() => Some(UrlKind::Clip(id.to_string())),
                // twitch.tv/<channel>
                [channel] if is_valid_username(channel) => {
                    Some(UrlKind::Stream(channel.to_string()))
                }
                _ => None,
            }
        } else if host == "clips.twitch.tv" {
            // clips.twitch.tv/<id>
            match segments.as_slice() {
                [id] if !id.is_empty() => Some(UrlKind::Clip(id.to_string())),
                _ => None,
            }
        } else {
            None
        }
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

/// Checks if a string looks like a valid Twitch username.
///
/// Twitch usernames are 4-25 characters long and contain alphanumeric characters
/// and underscores.
fn is_valid_username(s: &str) -> bool {
    let len = s.len();
    (4..=25).contains(&len) && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings() {
        let settings = Settings::default();

        assert!(settings.client_id.is_none());
        assert!(settings.client_secret.is_none());
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "client_id": "id",
            "client_secret": "secret",
        }))
        .expect("could not deserialize settings");

        assert_eq!(settings.client_id.as_deref(), Some("id"));
        assert_eq!(settings.client_secret.as_deref(), Some("secret"));
    }
}
