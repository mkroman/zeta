//! Expands Chaturbate room links with broadcaster details.
//!
//! Links to a broadcaster room on `chaturbate.com` are answered with the broadcaster's
//! username, gender, and room title — or just the username with an offline mark when the room
//! is not public. The details are scraped from the room page's `window.initialRoomDossier`
//! JSON blob rather than an API; the plugin has no settings.

use regex::Regex;
use serde::Deserialize;
use tracing::debug;
use url::Url;

use crate::{
    config::HttpConfig,
    error::RequestError,
    http,
    plugin::prelude::*,
    url::{is_identifier, path_segments},
};

/// The Chaturbate hosts whose links this plugin handles.
const URL_HOSTS: &[&str] = &["chaturbate.com", "www.chaturbate.com"];

/// Plugin for handling Chaturbate URLs and fetching broadcaster room info.
pub struct Chaturbate {
    client: reqwest::Client,
    room_dossier_re: Regex,
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
    /// A zeta error occurred while sending the reply.
    #[error("zeta error: {0}")]
    Zeta(#[from] ZetaError),
    /// An irc error occurred while sending the reply.
    #[error("irc error: {0}")]
    Irc(#[from] irc::error::Error),
}

impl From<Error> for ZetaError {
    fn from(err: Error) -> Self {
        ZetaError::Plugin(Box::new(err))
    }
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

#[async_trait]
impl Plugin<Context> for Chaturbate {
    type Settings = NoSettings;

    fn new(ctx: &Context, _settings: &NoSettings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.urls(UrlScope::Hosts(URL_HOSTS));

        Ok(Self::new(&ctx.config.http))
    }

    async fn handle_url(&self, _ctx: &Context, client: &Client, url: &UrlEvent) -> Result<(), ZetaError> {
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
    ///
    /// # Panics
    ///
    /// Panics if the room dossier extraction regex fails to compile, which can only happen on a
    /// programming error since the pattern is a constant.
    #[must_use]
    pub fn new(config: &HttpConfig) -> Self {
        let client = http::build_client(config);
        // The dossier is assigned as a JSON-encoded string literal, terminated by a semicolon
        // before the closing </script> tag.
        let room_dossier_re =
            Regex::new(r#"window\.initialRoomDossier\s*=\s*("(?:[^"\\]|\\.)*");"#).unwrap();

        Self {
            client,
            room_dossier_re,
        }
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

        let dossier = parse_room_dossier_with_re(&self.room_dossier_re, &html)?;
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

fn parse_room_dossier_with_re(re: &Regex, html: &str) -> Result<RoomDossier, Error> {
    let caps = re.captures(html).ok_or(Error::DossierNotFound)?;
    // The captured group is the outer JSON string, e.g. `"{ ... }"`.
    let outer: String = serde_json::from_str(&caps[1])?;
    // The inner value is the actual JSON object.
    let dossier: RoomDossier = serde_json::from_str(&outer)?;

    Ok(dossier)
}

/// Path segments that name site resources rather than broadcaster rooms.
const RESERVED_SEGMENTS: &[&str] = &[
    "auth", "affiliates", "tags", "search", "followed-cams", "new-cams", "female-cams",
    "male-cams", "couple-cams", "trans-cams",
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

    fn fixture_html(name: &str) -> String {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/chaturbate/{name}.html"));
        std::fs::read_to_string(path).expect("fixture file not found")
    }

    #[test]
    fn test_parse_room_dossier_fiery_redhead() {
        let html = fixture_html("fiery_redhead");
        let plugin = Chaturbate::new(&HttpConfig::default());
        let dossier = parse_room_dossier_with_re(&plugin.room_dossier_re, &html)
            .expect("should parse dossier");

        assert_eq!(dossier.broadcaster_username, "fiery_redhead");
        assert_eq!(dossier.room_status, "public");
        assert_eq!(dossier.broadcaster_gender, "female");
        assert_eq!(
            dossier.room_title,
            "start domi [12 tokens left] Prvts open :) #lush #redhead #bigbooty #natural"
        );
    }

    #[test]
    fn test_parse_room_dossier_milabunny() {
        let html = fixture_html("milabunny_");
        let plugin = Chaturbate::new(&HttpConfig::default());
        let dossier = parse_room_dossier_with_re(&plugin.room_dossier_re, &html)
            .expect("should parse dossier");

        assert_eq!(dossier.broadcaster_username, "milabunny_");
        assert_eq!(dossier.room_status, "public");
        assert_eq!(dossier.broadcaster_gender, "female");
        assert_eq!(
            dossier.room_title,
            "GOAL: Undress me to the end 🐰💞 / Hello, my name is Mila, I want to give you my smile and fun.🐰💕 #new #blonde #bigboobs #shy #18 [1424 tokens remaining]"
        );
    }

    #[test]
    fn test_extract_username_valid() {
        let cases = [
            (
                "https://chaturbate.com/fiery_redhead/",
                Some("fiery_redhead"),
            ),
            ("https://www.chaturbate.com/some_user", Some("some_user")),
        ];

        for (url_str, expected) in cases {
            let url = Url::parse(url_str).unwrap();
            assert_eq!(
                extract_username(&url).as_deref(),
                expected,
                "url: {url_str}"
            );
        }
    }

    #[test]
    fn test_extract_username_excluded_paths() {
        let cases = [
            "https://chaturbate.com/auth/login/",
            "https://chaturbate.com/tags/redhead/",
            "https://chaturbate.com/search/",
        ];

        for url_str in cases {
            let url = Url::parse(url_str).unwrap();
            assert_eq!(extract_username(&url), None, "url: {url_str}");
        }
    }

    #[test]
    fn test_extract_username_wrong_host() {
        let url = Url::parse("https://example.com/user/").unwrap();
        assert_eq!(extract_username(&url), None);
    }
}
