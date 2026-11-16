//! Recognition of Spotify links and bare `spotify:type:id` URIs.
//!
//! Both parsers return the resource type and id, leaving the dispatch and reply to
//! [`crate::commands`].

use url::Url;

/// The Spotify hosts whose links this plugin handles.
pub(super) const URL_HOSTS: &[&str] = &["open.spotify.com", "play.spotify.com"];

/// Returns the bare `spotify:type:id` URIs found in `text`, in order.
pub(super) fn parse_spotify_uris(text: &str) -> Vec<(&str, &str)> {
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

/// Returns the resource type and id of a Spotify link, if the path is exactly `type/id`.
pub(super) fn parse_spotify_url(url: &Url) -> Option<(&str, &str)> {
    // path segments: ["track", "4uLU6hMCjMI75M1A2tKUQC"]
    match zeta_url::path_segments(url)?.as_slice() {
        [kind, id] => Some((kind, id)),
        _ => None,
    }
}
