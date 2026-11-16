//! Fetching and replying to a Spotify resource.
//!
//! The event handlers in the crate root parse a link or bare URI into a resource type and id
//! and dispatch here. Each resource type renders its own reply, with the external `spotify:`
//! URL appended only for URI matches; fetch failures are mapped to a reply-grade sentence for
//! missing resources.

use std::fmt::Write;

use serde::Deserialize;
use tracing::{debug, warn};

use zeta_http as http;
use zeta_http::reqwest;
use zeta_http::reqwest::header::AUTHORIZATION;
use zeta_plugin::irc;
use zeta_plugin::irc::{labeled_reply, send_reply};
use zeta_plugin::{Client, ZetaError};
use zeta_utils::text::{self, ListJoinExt, format_number};

use super::model::{Album, Artist, ArtistSimple, Playlist, Track};
use super::token;
use super::{Error, Spotify};

/// The Spotify Web API base URL.
const API_BASE_URL: &str = "https://api.spotify.com/v1";

impl Spotify {
    #[allow(clippy::too_many_lines)] // inlined per-type renders keep the reply bytes obvious
    pub(super) async fn handle_spotify_resource(
        &self,
        channel: &str,
        type_str: &str,
        id_str: &str,
        include_url: bool,
        client: &Client,
    ) -> Result<(), ZetaError> {
        match type_str {
            "track" => {
                self.send_details(
                    &format!("tracks/{id_str}"),
                    channel,
                    include_url,
                    client,
                    |track: &Track, include_url| {
                        let name = irc::unstyle_text(&track.name);
                        let artists = join_artists(&track.artists);
                        let album = irc::unstyle_text(&track.album.name);

                        let mut msg = format!("\x0f{name}\x0310 is a track by {artists}\x0310");
                        let _ = write!(msg, " from the album \x0f{album}\x0310");

                        if include_url {
                            let _ = write!(msg, " - {}", track.external_urls.spotify);
                        }

                        msg
                    },
                )
                .await
            }
            "album" => {
                self.send_details(
                    &format!("albums/{id_str}"),
                    channel,
                    include_url,
                    client,
                    |album: &Album, include_url| {
                        let name = irc::unstyle_text(&album.name);
                        let artists = join_artists(&album.artists);

                        let mut msg = format!("\x0f{name}\x0310 is an album by {artists}");

                        if include_url {
                            let _ = write!(msg, " - {}", album.external_urls.spotify);
                        }

                        msg
                    },
                )
                .await
            }
            "artist" => {
                self.send_details(
                    &format!("artists/{id_str}"),
                    channel,
                    include_url,
                    client,
                    |artist: &Artist, include_url| {
                        let name = irc::unstyle_text(&artist.name);
                        let genres: Vec<_> = artist.genres.iter().map(|genre| irc::unstyle_text(genre)).collect();
                        let joined = genres.oxford_join();
                        let followers = format_number(artist.followers.total);
                        let follower_noun = text::pluralize_noun(artist.followers.total, "follower");

                        let mut msg = format!("\x0f{name}\x0310 is");
                        if genres.is_empty() {
                            let _ = write!(msg, " an");
                        } else {
                            let article = text::indefinite_article(&joined);
                            let _ = write!(msg, " {article} {joined}");
                        }
                        let _ = write!(
                            msg,
                            " artist with \x0f{followers}\x0310 {follower_noun}"
                        );

                        if include_url {
                            let _ = write!(msg, " - {}", artist.external_urls.spotify);
                        }

                        msg
                    },
                )
                .await
            }
            "playlist" => {
                self.send_details(
                    &format!("playlists/{id_str}"),
                    channel,
                    include_url,
                    client,
                    |playlist: &Playlist, include_url| {
                        let name = irc::unstyle_text(&playlist.name);
                        let total_tracks = format_number(playlist.tracks.total);
                        let track_noun = text::pluralize_noun(playlist.tracks.total, "track");
                        let owner = irc::unstyle_text(
                            playlist
                                .owner
                                .display_name
                                .as_deref()
                                .unwrap_or(&playlist.owner.id),
                        );
                        let followers = playlist.followers.total;

                        let mut msg = format!(
                            "\x0f{name}\x0310 is a playlist with\x0f {total_tracks}\x0310 {track_noun} curated by\x0f {owner}\x0310"
                        );

                        if followers > 0 {
                            let followers_fmt = format_number(followers);
                            let follower_noun = text::pluralize_noun(followers, "follower");
                            let _ = write!(
                                msg,
                                " with \x0f{followers_fmt}\x0310 {follower_noun}"
                            );
                        }

                        if include_url {
                            let _ = write!(msg, " - {}", playlist.external_urls.spotify);
                        }

                        msg
                    },
                )
                .await
            }
            _ => {
                debug!(spotify.resource.type = %type_str, "unsupported spotify type");
                Ok(())
            }
        }
    }

    async fn fetch<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, Error> {
        let token = self
            .credentials
            .access_token_shared(token::token_grant)
            .await?;
        let url = format!("{API_BASE_URL}/{path}");

        let request = self
            .credentials
            .client()
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {token}"));
        http::get_json(request).await
    }

    /// Fetches a resource, formats it, and replies with the message.
    ///
    /// `render` receives the fetched details and whether the reply should include the external
    /// URL. Fetch errors are reported through [`handle_error`].
    async fn send_details<T, F>(
        &self,
        path: &str,
        channel: &str,
        include_url: bool,
        client: &Client,
        render: F,
    ) -> Result<(), ZetaError>
    where
        T: for<'de> Deserialize<'de>,
        F: FnOnce(&T, bool) -> String,
    {
        match self.fetch::<T>(path).await {
            Ok(details) => {
                send_reply(
                    client,
                    channel,
                    labeled_reply("Spotify", render(&details, include_url)),
                )?;
            }
            Err(error) => handle_error(channel, client, &error)?,
        }

        Ok(())
    }
}

fn handle_error(channel: &str, client: &Client, error: &Error) -> Result<(), ZetaError> {
    warn!(exception.message = %error, "spotify error");
    if let http::ApiError::Status { status, .. } = error
        && *status == reqwest::StatusCode::NOT_FOUND
    {
        send_reply(
            client,
            channel,
            labeled_reply("Spotify", "Resource not found"),
        )?;
    }

    Ok(())
}

pub(super) fn join_artists(artists: &[ArtistSimple]) -> String {
    let names: Vec<String> = artists
        .iter()
        .map(|a| format!("\x0f{}\x0310", irc::unstyle_text(&a.name)))
        .collect();
    names.oxford_join()
}
