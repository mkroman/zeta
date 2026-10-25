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

use std::fmt::Write;

use base64::prelude::*;
use indefinite::indefinite_article_only;
use num_format::{Locale, ToFormattedString};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use url::Url;

use crate::prelude::*;
use zeta_http as http;
use zeta_http::oauth::Credentials;
use zeta_url::path_segments;

/// The Spotify hosts whose links this plugin handles.
const URL_HOSTS: &[&str] = &["open.spotify.com", "play.spotify.com"];

const AUTH_URL: &str = "https://accounts.spotify.com/api/token";
const API_BASE_URL: &str = "https://api.spotify.com/v1";

/// Settings for the spotify plugin, from its `[plugins.spotify]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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
    credentials: Credentials,
}

/// Errors that can occur while talking to the Spotify API.
pub type Error = http::ApiError;

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

#[plugin(
    name = "spotify",
    settings = Settings,
    urls(hosts = URL_HOSTS),
    messages
)]
impl Spotify {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client_id = resolve_secret(settings.client_id.as_deref(), "SPOTIFY_CLIENT_ID")?;
        let client_secret =
            resolve_secret(settings.client_secret.as_deref(), "SPOTIFY_CLIENT_SECRET")?;
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

        // Handle Spotify URLs (open.spotify.com/type/id); do not include an external URL for
        // link matches, avoiding redundancy.
        if let Some((type_str, id_str)) = parse_spotify_url(url.url()) {
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
        for (type_str, id_str) in parse_spotify_uris(event.text()) {
            // Include the external URL for URI matches.
            self.handle_spotify_resource(channel, type_str, id_str, true, client)
                .await?;
        }

        Ok(())
    }
}

impl Spotify {
    /// Builds the token request for the Spotify client-credentials grant, authenticating the
    /// application with a Basic authorization header.
    fn token_grant(client: &reqwest::Client, credentials: &Credentials) -> reqwest::RequestBuilder {
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
        let token = self.credentials.access_token(Self::token_grant).await?;
        let url = format!("{API_BASE_URL}/{path}");

        let request = self
            .credentials
            .client()
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {token}"));
        http::get_json(request).await
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
                    let article = indefinite_article_only(&genres);
                    let _ = write!(msg, " {article} {genres}");
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
    if let http::ApiError::Status { status, .. } = error
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
    match words.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, [first])) => format!("{first} and {last}"),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// Returns the bare `spotify:type:id` URIs found in `text`, in order.
fn parse_spotify_uris(text: &str) -> Vec<(&str, &str)> {
    const PREFIX: &str = "spotify:";

    let mut uris = Vec::new();
    let mut cursor = 0;

    while let Some(offset) = text[cursor..].find(PREFIX) {
        let start = cursor + offset;
        let rest = &text[start + PREFIX.len()..];
        let Some(colon) = rest.find(':') else { break };

        let kind = &rest[..colon];
        let tail = &rest[colon + 1..];
        let id_len = tail.bytes().take_while(u8::is_ascii_alphanumeric).count();

        if !kind.is_empty() && kind.bytes().all(|byte| byte.is_ascii_alphabetic()) && id_len > 0 {
            uris.push((kind, &tail[..id_len]));
            cursor = start + PREFIX.len() + colon + 1 + id_len;
        } else {
            cursor = start + PREFIX.len();
        }
    }

    uris
}

fn parse_spotify_url(url: &Url) -> Option<(&str, &str)> {
    // path segments: ["track", "4uLU6hMCjMI75M1A2tKUQC"]
    match path_segments(url)?.as_slice() {
        [kind, id] => Some((kind, id)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use similar_asserts::assert_eq;
    use zeta_test_support::settings_tests;
    use zeta_utils::irc::strip_formatting;

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

    #[test]
    fn parses_bare_spotify_uris() {
        assert_eq!(
            parse_spotify_uris(
                "listen to spotify:track:4uLU6hMCjMI75M1A2tKUQC and \
                 spotify:album:1DFixLWuPkv3KT3TnV35m3"
            ),
            [
                ("track", "4uLU6hMCjMI75M1A2tKUQC"),
                ("album", "1DFixLWuPkv3KT3TnV35m3"),
            ]
        );

        // Each component must be present and use the URI alphabet.
        for text in [
            "spotify::id",
            "spotify:track:",
            "spotify:tr4ck:id",
            // Plain links are handled by `parse_spotify_url`, not the URI parser.
            "https://open.spotify.com/track/id",
        ] {
            assert_eq!(
                parse_spotify_uris(text),
                Vec::<(&str, &str)>::new(),
                "{text}"
            );
        }

        // The id ends at the first character outside the alphabet.
        assert_eq!(
            parse_spotify_uris("spotify:track:abc! rest"),
            [("track", "abc")]
        );

        // Matches are non-overlapping.
        assert_eq!(
            parse_spotify_uris("spotify:spotify:track:id"),
            [("spotify", "track")]
        );
    }

    #[test]
    fn parses_spotify_resource_urls() {
        fn parse(url: &str) -> Option<(String, String)> {
            parse_spotify_url(&Url::parse(url).unwrap())
                .map(|(kind, id)| (kind.to_string(), id.to_string()))
        }

        assert_eq!(
            parse("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC"),
            Some(("track".to_string(), "4uLU6hMCjMI75M1A2tKUQC".to_string()))
        );
        assert_eq!(
            parse("https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3?si=abc"),
            Some(("album".to_string(), "1DFixLWuPkv3KT3TnV35m3".to_string()))
        );

        // Root paths and deeper nesting are not resources.
        assert_eq!(parse("https://open.spotify.com/"), None);
        assert_eq!(
            parse("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC/extra"),
            None
        );
    }

    #[test]
    fn joins_artists_into_a_sentence() {
        let artists = |names: &[&str]| {
            names
                .iter()
                .map(|name| ArtistSimple {
                    name: (*name).to_string(),
                })
                .collect::<Vec<_>>()
        };

        let cases = [
            ("empty", &[][..], ""),
            ("single", &["Aurora"][..], "Aurora"),
            ("pair", &["Aurora", "Sigrid"][..], "Aurora and Sigrid"),
        ];

        for (case, names, expected) in cases {
            assert_eq!(
                strip_formatting(&join_artists(&artists(names))),
                expected,
                "{case}"
            );
        }

        // The comma sits between two color codes, so `strip_formatting` would swallow it; pin the exact
        // layout for the three-name variant to keep the separator covered.
        assert_eq!(
            join_artists(&artists(&["Aurora", "Sigrid", "Agnes Obel"])),
            "\x0fAurora\x0310, \x0fSigrid\x0310 and \x0fAgnes Obel\x0310"
        );
    }
}
