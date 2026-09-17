use std::fmt::Write;

use base64::prelude::*;
use num_format::{Locale, ToFormattedString};
use regex::Regex;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use url::Url;

use crate::{
    http,
    oauth::{TokenCache, TokenResponse},
    plugin::prelude::*,
};

/// The Spotify hosts whose links this plugin handles.
const URL_HOSTS: &[&str] = &["open.spotify.com", "play.spotify.com"];

const AUTH_URL: &str = "https://accounts.spotify.com/api/token";
const API_BASE_URL: &str = "https://api.spotify.com/v1";

/// Settings for the spotify plugin, from its `[plugins.spotify]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The Spotify application client id.
    ///
    /// Falls back to the `SPOTIFY_CLIENT_ID` environment variable when unset.
    #[serde(default)]
    pub client_id: Option<String>,
    /// The Spotify application client secret.
    ///
    /// Falls back to the `SPOTIFY_CLIENT_SECRET` environment variable when unset.
    #[serde(default)]
    pub client_secret: Option<String>,
}

/// Spotify integration plugin.
pub struct Spotify {
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    token: TokenCache,
    uri_regex: Regex,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error(transparent)]
    Api(#[from] http::ApiError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Deserialize)]
struct Track {
    name: String,
    artists: Vec<ArtistSimple>,
    album: AlbumSimple,
    external_urls: ExternalUrls,
}

#[derive(Deserialize)]
struct Album {
    name: String,
    artists: Vec<ArtistSimple>,
    external_urls: ExternalUrls,
}

#[derive(Deserialize)]
struct Artist {
    name: String,
    genres: Vec<String>,
    followers: Followers,
    external_urls: ExternalUrls,
}

#[derive(Deserialize)]
struct Playlist {
    name: String,
    owner: Owner,
    followers: Followers,
    tracks: PlaylistTracks,
    external_urls: ExternalUrls,
}

#[derive(Deserialize)]
struct ArtistSimple {
    name: String,
}

#[derive(Deserialize)]
struct AlbumSimple {
    name: String,
}

#[derive(Deserialize)]
struct ExternalUrls {
    spotify: String,
}

#[derive(Deserialize)]
struct Followers {
    total: u64,
}

#[derive(Deserialize)]
struct Owner {
    display_name: Option<String>,
    id: String,
}

#[derive(Deserialize)]
struct PlaylistTracks {
    total: u64,
}

#[async_trait]
impl Plugin<Context> for Spotify {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client_id = resolve_secret(settings.client_id.as_deref(), "SPOTIFY_CLIENT_ID")?;
        let client_secret =
            resolve_secret(settings.client_secret.as_deref(), "SPOTIFY_CLIENT_SECRET")?;
        let client = http::build_client(&ctx.config.http);
        let uri_regex = Regex::new(r"spotify:(?P<type>[a-zA-Z]+):(?P<id>[a-zA-Z0-9]+)").unwrap();

        Ok(Self {
            client,
            client_id,
            client_secret,
            token: TokenCache::new(),
            uri_regex,
        })
    }

    fn url_hosts(&self) -> &'static [&'static str] {
        URL_HOSTS
    }

    async fn handle_message(
        &self,
        ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        let Command::PRIVMSG(channel, user_message) = &message.command else {
            return Ok(());
        };

        // 1. Handle Spotify URIs (spotify:type:id)
        for cap in self.uri_regex.captures_iter(user_message) {
            let type_str = &cap["type"];
            let id_str = &cap["id"];
            // Include external URL for URI matches
            self.handle_spotify_resource(channel, type_str, id_str, true, client)
                .await?;
        }

        // 2. Handle Spotify URLs (open.spotify.com/type/id)
        for url in FilteredUrls::from_message(ctx, message)
            .into_iter()
            .flatten()
        {
            if let Some(host) = url.host_str()
                && URL_HOSTS.contains(&host)
                && let Some((type_str, id_str)) = parse_spotify_url(&url)
            {
                // Do not include external URL for link matches (avoid redundancy)
                self.handle_spotify_resource(channel, type_str, id_str, false, client)
                    .await?;
            }
        }

        Ok(())
    }
}

impl Spotify {
    /// Authenticates with Spotify using Client Credentials Flow.
    async fn get_token(&self) -> Result<String, Error> {
        self.token
            .get(|| async {
                debug!("refreshing spotify token");
                let creds = format!("{}:{}", self.client_id, self.client_secret);
                let encoded = BASE64_STANDARD.encode(creds);

                self.client
                    .post(AUTH_URL)
                    .header(AUTHORIZATION, format!("Basic {encoded}"))
                    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .form(&[("grant_type", "client_credentials")])
                    .send()
                    .await?
                    .json::<TokenResponse>()
                    .await
                    .map_err(Error::from)
            })
            .await
    }

    async fn handle_spotify_resource(
        &self,
        channel: &str,
        type_str: &str,
        id_str: &str,
        include_url: bool,
        client: &Client,
    ) -> Result<(), ZetaError> {
        match type_str {
            "track" => {
                self.send_track_details(channel, id_str, include_url, client)
                    .await
            }
            "album" => {
                self.send_album_details(channel, id_str, include_url, client)
                    .await
            }
            "artist" => {
                self.send_artist_details(channel, id_str, include_url, client)
                    .await
            }
            "playlist" => {
                self.send_playlist_details(channel, id_str, include_url, client)
                    .await
            }
            _ => {
                debug!("Unsupported spotify type: {}", type_str);
                Ok(())
            }
        }
    }

    async fn fetch<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, Error> {
        let token = self.get_token().await?;
        let url = format!("{API_BASE_URL}/{path}");

        let response = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .send()
            .await?;

        http::parse_response(response).await.map_err(Error::from)
    }

    async fn send_track_details(
        &self,
        channel: &str,
        id: &str,
        include_url: bool,
        client: &Client,
    ) -> Result<(), ZetaError> {
        match self.fetch::<Track>(&format!("tracks/{id}")).await {
            Ok(track) => {
                let name = track.name;
                let artists = join_artists(&track.artists);
                let album = track.album.name;

                let mut msg = format!("\x0f{name}\x0310 is a track by {artists}\x0310");
                let _ = write!(msg, " from the album \x0f{album}\x0310");

                if include_url {
                    let _ = write!(msg, " - {}", track.external_urls.spotify);
                }

                client.send_privmsg(channel, reply("Spotify", &msg))?;
            }
            Err(e) => handle_error(channel, client, &e)?,
        }
        Ok(())
    }

    async fn send_album_details(
        &self,
        channel: &str,
        id: &str,
        include_url: bool,
        client: &Client,
    ) -> Result<(), ZetaError> {
        match self.fetch::<Album>(&format!("albums/{id}")).await {
            Ok(album) => {
                let name = album.name;
                let artists = join_artists(&album.artists);

                let mut msg = format!("\x0f{name}\x0310 is an album by {artists}");

                if include_url {
                    let _ = write!(msg, " - {}", album.external_urls.spotify);
                }

                client.send_privmsg(channel, reply("Spotify", &msg))?;
            }
            Err(e) => handle_error(channel, client, &e)?,
        }
        Ok(())
    }

    async fn send_artist_details(
        &self,
        channel: &str,
        id: &str,
        include_url: bool,
        client: &Client,
    ) -> Result<(), ZetaError> {
        match self.fetch::<Artist>(&format!("artists/{id}")).await {
            Ok(artist) => {
                let name = artist.name;
                let genres = to_sentence(&artist.genres);
                let followers = artist.followers.total.to_formatted_string(&Locale::en);

                let mut msg = format!("\x0f{name}\x0310 is");
                if artist.genres.is_empty() {
                    let _ = write!(msg, " an");
                } else {
                    let _ = write!(msg, " a {genres}");
                }
                let _ = write!(msg, " artist with \x0f{followers}\x0310 followers");

                if include_url {
                    let _ = write!(msg, " - {}", artist.external_urls.spotify);
                }

                client.send_privmsg(channel, reply("Spotify", &msg))?;
            }
            Err(e) => handle_error(channel, client, &e)?,
        }
        Ok(())
    }

    async fn send_playlist_details(
        &self,
        channel: &str,
        id: &str,
        include_url: bool,
        client: &Client,
    ) -> Result<(), ZetaError> {
        match self.fetch::<Playlist>(&format!("playlists/{id}")).await {
            Ok(playlist) => {
                let name = playlist.name;
                let total_tracks = playlist.tracks.total;
                let owner = playlist.owner.display_name.unwrap_or(playlist.owner.id);
                let followers = playlist.followers.total;

                let mut msg = format!(
                    "\x0f{name}\x0310 is a playlist with\x0f {total_tracks}\x0310 tracks curated by\x0f {owner}\x0310"
                );

                if followers > 0 {
                    let followers_fmt = followers.to_formatted_string(&Locale::en);
                    let _ = write!(msg, " with \x0f{followers_fmt}\x0310 followers");
                }

                if include_url {
                    let _ = write!(msg, " - {}", playlist.external_urls.spotify);
                }

                client.send_privmsg(channel, reply("Spotify", &msg))?;
            }
            Err(e) => handle_error(channel, client, &e)?,
        }
        Ok(())
    }
}

fn handle_error(channel: &str, client: &Client, error: &Error) -> Result<(), ZetaError> {
    warn!("Spotify error: {}", error);
    // Mimic Ruby behavior: simplistic error messages for common HTTP codes could be added here
    // For now, we generally don't spam the channel with errors unless it's critical,
    // but the Ruby plugin did print "Invalid track ID" etc.
    if let Error::Api(http::ApiError::Status(status)) = error
        && *status == reqwest::StatusCode::NOT_FOUND
    {
        client.send_privmsg(channel, reply("Spotify", "Resource not found"))?;
    }

    Ok(())
}

fn join_artists(artists: &[ArtistSimple]) -> String {
    let names: Vec<String> = artists
        .iter()
        .map(|a| format!("\x0f{}\x0310", a.name))
        .collect();
    to_sentence(&names)
}

fn to_sentence(words: &[String]) -> String {
    match words.len() {
        0 => String::new(),
        1 => words[0].clone(),
        2 => format!("{} and {}", words[0], words[1]),
        _ => {
            let last = words.last().unwrap();
            let others = words[..words.len() - 1].join(", ");
            format!("{others} and {last}")
        }
    }
}

fn parse_spotify_url(url: &Url) -> Option<(&str, &str)> {
    // path segments: ["track", "4uLU6hMCjMI75M1A2tKUQC"]
    let segments: Vec<&str> = url.path_segments()?.collect();
    if segments.len() >= 2 {
        Some((segments[0], segments[1]))
    } else {
        None
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
