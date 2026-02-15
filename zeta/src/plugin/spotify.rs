use std::env;
use std::fmt::Write;
use std::time::{Duration, Instant};

use base64::prelude::*;
use num_format::{Locale, ToFormattedString};
use regex::Regex;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, warn};
use url::Url;

use crate::{
    http,
    plugin::{self, prelude::*},
};

const AUTH_URL: &str = "https://accounts.spotify.com/api/token";
const API_BASE_URL: &str = "https://api.spotify.com/v1";

/// Spotify integration plugin.
pub struct Spotify {
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    token: RwLock<Option<Token>>,
    uri_regex: Regex,
}

#[derive(Clone, Debug)]
struct Token {
    access_token: String,
    expires_at: Instant,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("api error: {0}")]
    Api(String),
}

#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
    expires_in: u64,
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
    fn new(_ctx: &Context) -> Self {
        let client_id =
            env::var("SPOTIFY_CLIENT_ID").expect("missing SPOTIFY_CLIENT_ID environment variable");
        let client_secret = env::var("SPOTIFY_CLIENT_SECRET")
            .expect("missing SPOTIFY_CLIENT_SECRET environment variable");
        let client = http::build_client();
        let uri_regex = Regex::new(r"spotify:(?P<type>[a-zA-Z]+):(?P<id>[a-zA-Z0-9]+)").unwrap();

        Self {
            client,
            client_id,
            client_secret,
            token: RwLock::new(None),
            uri_regex,
        }
    }

    fn name() -> Name {
        Name::from("spotify")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("0.3")
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command {
            // 1. Handle Spotify URIs (spotify:type:id)
            for cap in self.uri_regex.captures_iter(user_message) {
                let type_str = &cap["type"];
                let id_str = &cap["id"];
                // Include external URL for URI matches
                self.handle_spotify_resource(channel, type_str, id_str, true, client)
                    .await?;
            }

            // 2. Handle Spotify URLs (open.spotify.com/type/id)
            if let Some(urls) = plugin::extract_urls(user_message) {
                for url in urls {
                    if let Some(host) = url.host_str()
                        && (host == "open.spotify.com" || host == "play.spotify.com")
                        && let Some((type_str, id_str)) = parse_spotify_url(&url)
                    {
                        // Do not include external URL for link matches (avoid redundancy)
                        self.handle_spotify_resource(channel, type_str, id_str, false, client)
                            .await?;
                    }
                }
            }
        }

        Ok(())
    }
}

impl Spotify {
    /// Authenticates with Spotify using Client Credentials Flow.
    async fn get_token(&self) -> Result<String, Error> {
        // Check cache
        if let Some(token) = self.token.read().await.as_ref()
            && token.expires_at > Instant::now() + Duration::from_secs(60)
        {
            return Ok(token.access_token.clone());
        }

        debug!("refreshing spotify token");
        let creds = format!("{}:{}", self.client_id, self.client_secret);
        let encoded = BASE64_STANDARD.encode(creds);
        let response = self
            .client
            .post(AUTH_URL)
            .header(AUTHORIZATION, format!("Basic {encoded}"))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await?;

        let auth: AuthResponse = response.json().await?;
        let token = Token {
            access_token: auth.access_token.clone(),
            expires_at: Instant::now() + Duration::from_secs(auth.expires_in),
        };

        *self.token.write().await = Some(token);

        Ok(auth.access_token)
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

        if !response.status().is_success() {
            return Err(Error::Api(format!("status: {}", response.status())));
        }

        Ok(response.json().await?)
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

                client.send_privmsg(channel, formatted(&msg))?;
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

                client.send_privmsg(channel, formatted(&msg))?;
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

                client.send_privmsg(channel, formatted(&msg))?;
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

                client.send_privmsg(channel, formatted(&msg))?;
            }
            Err(e) => handle_error(channel, client, &e)?,
        }
        Ok(())
    }
}

fn formatted(message: &str) -> String {
    format!("\x0310>\x0f\x02 Spotify:\x02\x0310 {message}")
}

fn handle_error(channel: &str, client: &Client, error: &Error) -> Result<(), ZetaError> {
    warn!("Spotify error: {}", error);
    // Mimic Ruby behavior: simplistic error messages for common HTTP codes could be added here
    // For now, we generally don't spam the channel with errors unless it's critical,
    // but the Ruby plugin did print "Invalid track ID" etc.
    // Since we use reqwest, specific status codes like 404/400 would be inside Error::Request or Error::Api
    if let Error::Api(s) = error
        && s.contains("404")
    {
        client.send_privmsg(channel, formatted("Resource not found"))?;
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
