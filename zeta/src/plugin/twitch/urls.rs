//! Parsing of Twitch URLs.

use url::Url;

use crate::url::{is_id_segment, is_numeric_segment, path_segments};

/// The Twitch.tv hostname.
pub(super) const TWITCH_HOST: &str = "twitch.tv";

/// The www-prefixed Twitch.tv hostname.
pub(super) const TWITCH_WWW_HOST: &str = "www.twitch.tv";

/// The hostname of Twitch clip URLs.
pub(super) const CLIPS_HOST: &str = "clips.twitch.tv";

/// The Twitch hosts whose links this plugin handles.
pub(super) const URL_HOSTS: &[&str] = &[CLIPS_HOST, TWITCH_HOST, TWITCH_WWW_HOST];

/// The type of Twitch resource found in a URL.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum UrlKind {
    /// A live stream, identified by its channel login.
    Stream(String),
    /// A clip, identified by its id.
    Clip(String),
    /// A video (VOD), identified by its id.
    Video(String),
}

/// Parses the given `url` and returns a [`UrlKind`] depending on the type of Twitch URL,
/// returning `None` for URLs that match none of the resource kinds — including channel
/// subpaths.
#[must_use]
pub(super) fn parse_twitch_url(url: &Url) -> Option<UrlKind> {
    match url.host_str()? {
        TWITCH_HOST | TWITCH_WWW_HOST => parse_twitch_tv_url(url),
        CLIPS_HOST => parse_clips_url(url),
        _ => None,
    }
}

/// Path segments that name site resources rather than channel streams.
const RESERVED_SEGMENTS: &[&str] = &["clip", "videos"];

/// Parses twitch.tv URLs.
fn parse_twitch_tv_url(url: &Url) -> Option<UrlKind> {
    match path_segments(url)?.as_slice() {
        // twitch.tv/videos/<id>
        ["videos", id] if is_numeric_segment(id) => Some(UrlKind::Video((*id).to_string())),
        // twitch.tv/<channel>/clip/<id>
        [_, "clip", id] if is_valid_clip_id(id) => Some(UrlKind::Clip((*id).to_string())),
        // twitch.tv/<channel>
        [channel] if !RESERVED_SEGMENTS.contains(channel) && is_valid_username(channel) => {
            Some(UrlKind::Stream((*channel).to_string()))
        }
        _ => None,
    }
}

/// Parses clips.twitch.tv URLs.
fn parse_clips_url(url: &Url) -> Option<UrlKind> {
    match path_segments(url)?.as_slice() {
        // clips.twitch.tv/<id>
        [id] if is_valid_clip_id(id) => Some(UrlKind::Clip((*id).to_string())),
        _ => None,
    }
}

/// Returns whether the segment is a valid Twitch clip id (a base62 slug).
#[must_use]
fn is_valid_clip_id(id: &str) -> bool {
    is_id_segment(id, "")
}

/// Checks if a string looks like a valid Twitch username.
///
/// Twitch usernames are 4-25 characters long and contain alphanumeric characters and
/// underscores.
fn is_valid_username(s: &str) -> bool {
    (4..=25).contains(&s.len()) && is_id_segment(s, "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stream_urls() {
        let test_cases = [
            (
                "https://twitch.tv/lirik",
                Some(UrlKind::Stream("lirik".to_string())),
            ),
            (
                "https://www.twitch.tv/lirik/",
                Some(UrlKind::Stream("lirik".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_twitch_url(&url), expected, "for {url_str}");
        }
    }

    #[test]
    fn test_parse_video_urls() {
        let test_cases = [
            (
                "https://twitch.tv/videos/2119948564",
                Some(UrlKind::Video("2119948564".to_string())),
            ),
            (
                "https://www.twitch.tv/videos/2119948564/",
                Some(UrlKind::Video("2119948564".to_string())),
            ),
            // Video ids are numeric.
            ("https://twitch.tv/videos/abc123", None),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_twitch_url(&url), expected, "for {url_str}");
        }
    }

    #[test]
    fn test_parse_clip_urls() {
        let test_cases = [
            (
                "https://clips.twitch.tv/AbrasiveFurryDolphGJ7K8WfDb4g",
                Some(UrlKind::Clip("AbrasiveFurryDolphGJ7K8WfDb4g".to_string())),
            ),
            (
                "https://clips.twitch.tv/AbrasiveFurryDolphGJ7K8WfDb4g/",
                Some(UrlKind::Clip("AbrasiveFurryDolphGJ7K8WfDb4g".to_string())),
            ),
            (
                "https://twitch.tv/lirik/clip/AbrasiveFurryDolphGJ7K8WfDb4g",
                Some(UrlKind::Clip("AbrasiveFurryDolphGJ7K8WfDb4g".to_string())),
            ),
            // Clip ids are alphanumeric.
            ("https://clips.twitch.tv/ab;cd", None),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_twitch_url(&url), expected, "for {url_str}");
        }
    }

    #[test]
    fn test_invalid_urls() {
        let invalid_urls = [
            "https://example.com/lirik",
            "https://twitch.tv/",
            "https://twitch.tv/li;rik",
            "https://twitch.tv/videos/",
            "https://twitch.tv/lirik/videos/2119948564",
            "https://clips.twitch.tv/",
            "https://youtu.be/dQw4w9WgXcQ",
        ];

        for url_str in invalid_urls {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_twitch_url(&url), None, "for {url_str}");
        }
    }
}
