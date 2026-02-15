use std::env;
use std::time::{Duration, Instant};

use num_format::{Locale, ToFormattedString};
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, warn};
use url::Url;

use crate::{
    http,
    plugin::{self, prelude::*},
};

/// Twitch OAuth2 token endpoint.
const AUTH_URL: &str = "https://id.twitch.tv/oauth2/token";
/// Twitch Helix API base URL.
const BASE_URL: &str = "https://api.twitch.tv/helix";

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
    /// Cached access token.
    token: RwLock<Option<Token>>,
}

/// A Twitch OAuth2 access token.
#[derive(Clone, Debug)]
struct Token {
    /// The access token string.
    access_token: String,
    /// The time at which the token expires.
    expires_at: Instant,
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

/// Response from the Twitch OAuth2 token endpoint.
#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
    expires_in: u64,
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
    fn new(_ctx: &Context) -> Self {
        let client_id =
            env::var("TWITCH_CLIENT_ID").expect("missing TWITCH_CLIENT_ID environment variable");
        let client_secret = env::var("TWITCH_CLIENT_SECRET")
            .expect("missing TWITCH_CLIENT_SECRET environment variable");
        let client = http::build_client();

        Self {
            client,
            client_id,
            client_secret,
            token: RwLock::new(None),
        }
    }

    fn name() -> Name {
        Name::from("twitch")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("1.0")
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(urls) = plugin::extract_urls(user_message)
        {
            for url in urls {
                if let Some(kind) = self.parse_url(&url) {
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
        }

        Ok(())
    }
}

impl Twitch {
    /// Authenticates with Twitch using Client Credentials Flow.
    ///
    /// Returns a valid access token, refreshing it if necessary.
    async fn get_token(&self) -> Result<String, Error> {
        // Check if we have a valid cached token.
        if let Some(token) = self.token.read().await.as_ref() {
            // Add a 60 second buffer to the expiration time check.
            if token.expires_at > Instant::now() + Duration::from_secs(60) {
                return Ok(token.access_token.clone());
            }
        }

        debug!("refreshing twitch access token");
        let params = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("grant_type", "client_credentials"),
        ];

        let response = self.client.post(AUTH_URL).form(&params).send().await?;
        let auth: AuthResponse = response.error_for_status()?.json().await?;

        let token = Token {
            access_token: auth.access_token.clone(),
            expires_at: Instant::now() + Duration::from_secs(auth.expires_in),
        };

        *self.token.write().await = Some(token);

        Ok(auth.access_token)
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
    fn parse_url(&self, url: &Url) -> Option<UrlKind> {
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

            client.send_privmsg(channel, formatted(&format!(
                "{user_login}:\x0f {title}\x0310 - Game:\x0f {game_name}\x0310 Viewers:\x0f {viewers}\x0310"
            )))?;
        } else {
            // Fallback behavior: just print the channel name if not live.
            client.send_privmsg(channel, format!("\x0310> {user_login} - Twitch"))?;
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

            client.send_privmsg(channel, formatted(&format!(
                "“\x0f{title}\x0310” is a clip of\x0f {broadcaster}\x0310 clipped by\x0f {creator}\x0310 with\x0f {views}\x0310 views"
            )))?;
        } else {
            client.send_privmsg(channel, formatted("No results"))?;
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

            client.send_privmsg(channel, formatted(&format!(
                "“\x0f{title}\x0310” is a video by\x0f {user}\x0310 with\x0f {views}\x0310 views"
            )))?;
        } else {
            client.send_privmsg(channel, formatted("No results"))?;
        }

        Ok(())
    }
}

/// Formats a message with the Twitch prefix and colors.
fn formatted(message: &str) -> String {
    format!("\x0310>\x0F\x02 Twitch:\x02\x0310 {message}")
}

/// Checks if a string looks like a valid Twitch username.
///
/// Twitch usernames are 4-25 characters long and contain alphanumeric characters
/// and underscores.
fn is_valid_username(s: &str) -> bool {
    let len = s.len();
    (4..=25).contains(&len) && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}
