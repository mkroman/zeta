//! Expands Chaturbate room links with broadcaster details.
//!
//! Links to a broadcaster room on `chaturbate.com` are answered with the broadcaster's
//! username, gender, and room title — or just the username with an offline mark when the room
//! is not public. The details are scraped from the room page's `window.initialRoomDossier`
//! JSON blob rather than an API; the plugin has no settings.

use serde::Deserialize;
use tracing::debug;
use url::Url;

use crate::prelude::*;
use zeta_http as http;
use zeta_http::RequestError;
use zeta_plugin::config::HttpConfig;
use zeta_url::{is_identifier, path_segments};
use zeta_utils::text::quoted_string_literal;

/// The Chaturbate hosts whose links this plugin handles.
const URL_HOSTS: &[&str] = &["chaturbate.com", "www.chaturbate.com"];

/// Plugin for handling Chaturbate URLs and fetching broadcaster room info.
pub struct Chaturbate {
    client: reqwest::Client,
}

/// Errors that can occur when fetching or parsing a Chaturbate room.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Sending the HTTP request failed.
    #[error("request error: {0}")]
    Request(#[from] RequestError),
    /// The room page did not contain a room dossier.
    #[error("room dossier not found in page")]
    DossierNotFound,
    /// The room dossier could not be deserialized.
    #[error("failed to deserialize room dossier: {0}")]
    Deserialize(#[from] serde_json::Error),
    /// An irc error occurred while sending the reply.
    #[error("irc error: {0}")]
    Irc(#[from] irc::error::Error),
}

/// The parsed contents of `window.initialRoomDossier`.
#[derive(Deserialize, Debug)]
#[allow(unused)]
pub struct RoomDossier {
    /// The broadcaster's username.
    pub broadcaster_username: String,
    /// Current room status, e.g. `"public"` or `"offline"`.
    pub room_status: String,
    /// The room title / subject set by the broadcaster.
    pub room_title: String,
    /// The broadcaster's gender as reported by the site.
    pub broadcaster_gender: String,
}

#[plugin(name = "chaturbate", urls(hosts = URL_HOSTS))]
impl Chaturbate {
    fn new(ctx: &Context) -> Result<Self, ZetaError> {
        Ok(Self::with_http_config(&ctx.config.http))
    }

    #[handler(url)]
    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        url: &UrlEvent,
    ) -> Result<(), ZetaError> {
        let channel = url.channel();

        if let Some(username) = extract_username(url.url()) {
            debug!(%username, "processing chaturbate url");
            if let Err(e) = self.process_broadcaster(&username, channel, client).await {
                client.send_privmsg(channel, reply("Chaturbate", e.to_string()))?;
            }
        }

        Ok(())
    }
}

impl Chaturbate {
    /// Creates a new [`Chaturbate`] plugin instance.
    #[must_use]
    pub fn with_http_config(config: &HttpConfig) -> Self {
        let client = http::build_client(config);

        Self { client }
    }

    async fn process_broadcaster(
        &self,
        username: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let url = format!("https://chaturbate.com/{username}/");
        debug!(%url, "fetching chaturbate page");

        let response = http::send(self.client.get(&url)).await?;
        let html = http::text(response).await?;

        let dossier = parse_room_dossier(&html)?;
        debug!(?dossier, "parsed room dossier");

        let msg = if dossier.room_status == "offline" {
            format!("{} (\x0foffline\x0310)", dossier.broadcaster_username)
        } else {
            format!(
                "{} (\x0f{}\x0310) - {}",
                dossier.broadcaster_username, dossier.broadcaster_gender, dossier.room_title
            )
        };

        client.send_privmsg(channel, reply("Chaturbate", &msg))?;

        Ok(())
    }
}

fn parse_room_dossier(html: &str) -> Result<RoomDossier, Error> {
    const MARKER: &str = "window.initialRoomDossier";

    // The dossier is assigned as a JSON-encoded string literal, terminated by a semicolon
    // before the closing </script> tag.
    let tail = html.split_once(MARKER).ok_or(Error::DossierNotFound)?.1;
    let tail = tail
        .trim_start()
        .strip_prefix('=')
        .ok_or(Error::DossierNotFound)?
        .trim_start();
    let (literal, rest) = quoted_string_literal(tail).ok_or(Error::DossierNotFound)?;

    if !rest.starts_with(';') {
        return Err(Error::DossierNotFound);
    }

    let outer: String = serde_json::from_str(literal)?;

    Ok(serde_json::from_str(&outer)?)
}

/// Path segments that name site resources rather than broadcaster rooms.
const RESERVED_SEGMENTS: &[&str] = &[
    "auth",
    "affiliates",
    "tags",
    "search",
    "followed-cams",
    "new-cams",
    "female-cams",
    "male-cams",
    "couple-cams",
    "trans-cams",
];

/// Returns the broadcaster username from a Chaturbate URL, if present.
fn extract_username(url: &Url) -> Option<String> {
    let host = url.host_str()?;

    if !URL_HOSTS.contains(&host) {
        return None;
    }

    match path_segments(url)?.as_slice() {
        // Exclude well-known non-broadcaster paths.
        [username, ..] if is_identifier(username, "_") && !RESERVED_SEGMENTS.contains(username) => {
            Some((*username).to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::assert_parses;

    fn fixture_html(name: &str) -> String {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/chaturbate/{name}.html"));
        std::fs::read_to_string(path).expect("fixture file not found")
    }

    #[test]
    fn test_parse_room_dossier_fixtures() {
        let cases = [
            (
                "fiery_redhead",
                "fiery_redhead",
                "start domi [12 tokens left] Prvts open :) #lush #redhead #bigbooty #natural",
            ),
            (
                "milabunny_",
                "milabunny_",
                "GOAL: Undress me to the end 🐰💞 / Hello, my name is Mila, I want to give you my smile and fun.🐰💕 #new #blonde #bigboobs #shy #18 [1424 tokens remaining]",
            ),
        ];

        for (fixture, username, title) in cases {
            let html = fixture_html(fixture);
            let dossier = parse_room_dossier(&html).expect("should parse dossier");

            assert_eq!(dossier.broadcaster_username, username, "for {fixture}");
            assert_eq!(dossier.room_status, "public", "for {fixture}");
            assert_eq!(dossier.broadcaster_gender, "female", "for {fixture}");
            assert_eq!(dossier.room_title, title, "for {fixture}");
        }
    }

    #[test]
    fn parses_room_dossier_escapes_and_whitespace() {
        let html = r#"<script>
window.initialRoomDossier

    =

    "{\"broadcaster_username\":\"alice\",\"room_status\":\"public\",\"room_title\":\"say \\\"hi\\\"\",\"broadcaster_gender\":\"female\"}";
</script>"#;

        let dossier = parse_room_dossier(html).expect("should parse dossier");

        assert_eq!(dossier.broadcaster_username, "alice");
        assert_eq!(dossier.room_status, "public");
        assert_eq!(dossier.room_title, r#"say "hi""#);
        assert_eq!(dossier.broadcaster_gender, "female");
    }

    #[test]
    fn rejects_room_dossiers_without_a_quoted_literal() {
        for html in [
            "no dossier here",
            r#"window.initialRoomDossier "{\"room_status\":\"public\"}";"#,
            r"window.initialRoomDossier = 42;",
            r#"window.initialRoomDossier = "{\"room_status\":\"public\"}""#,
        ] {
            assert!(
                matches!(parse_room_dossier(html), Err(Error::DossierNotFound)),
                "{html}"
            );
        }
    }

    #[test]
    fn reports_deserialization_errors_after_the_literal() {
        let html = r#"window.initialRoomDossier = "not json";"#;

        assert!(matches!(
            parse_room_dossier(html),
            Err(Error::Deserialize(_))
        ));
    }

    #[test]
    fn test_extract_username() {
        assert_parses(
            extract_username,
            &[
                // Broadcaster rooms.
                (
                    "https://chaturbate.com/fiery_redhead/",
                    Some("fiery_redhead".to_string()),
                ),
                (
                    "https://www.chaturbate.com/some_user",
                    Some("some_user".to_string()),
                ),
                // Well-known non-broadcaster paths.
                ("https://chaturbate.com/auth/login/", None),
                ("https://chaturbate.com/tags/redhead/", None),
                ("https://chaturbate.com/search/", None),
                // Other hosts are not handled.
                ("https://example.com/user/", None),
            ],
        );
    }
}
