//! Parsing of YouTube URLs.

use url::Url;

use crate::url::{path_segments, query_param};

/// The hostname of shortened YouTube URLs.
const YOUTU_BE_HOST: &str = "youtu.be";

/// The YouTube.com hostname.
const YOUTUBE_COM_HOST: &str = "youtube.com";

/// The www-prefixed YouTube.com hostname.
const YOUTUBE_COM_WWW_HOST: &str = "www.youtube.com";

/// The YouTube hosts whose links this plugin handles.
pub(super) const URL_HOSTS: &[&str] = &[YOUTU_BE_HOST, YOUTUBE_COM_HOST, YOUTUBE_COM_WWW_HOST];

/// The kind of YouTube resource a URL points at.
#[derive(Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum UrlKind {
    /// Direct link to a video (e.g., `youtube.com/watch?v=VIDEO_ID` or `youtu.be/VIDEO_ID`)
    Video(String),
    /// Link to a short video (e.g., `youtube.com/shorts/VIDEO_ID`)
    Short(String),
    /// Direct link to a channel using channel ID (e.g., `youtube.com/channel/CHANNEL_ID`)
    Channel(String),
    /// Link to a channel using the @ handle (e.g., `youtube.com/@ChannelName`)
    ChannelHandle(String),
    /// Direct link to a playlist (e.g., `youtube.com/playlist?list=PLAYLIST_ID`)
    Playlist(String),
}

/// Parses the given `url` and returns a [`UrlKind`] depending on the type of YouTube URL.
#[must_use]
pub fn parse_youtube_url(url: &Url) -> Option<UrlKind> {
    match url.host_str()? {
        YOUTU_BE_HOST => parse_youtu_be_url(url),
        YOUTUBE_COM_HOST | YOUTUBE_COM_WWW_HOST => parse_youtube_com_url(url),
        _ => None,
    }
}

/// Parses youtube.com URLs
fn parse_youtube_com_url(url: &Url) -> Option<UrlKind> {
    match path_segments(url)?.as_slice() {
        // `/watch?v=<video_id>`
        ["watch"] => query_param(url, "v").map(UrlKind::Video),
        // `/playlist?list=<playlist_id>`
        ["playlist"] => query_param(url, "list").map(UrlKind::Playlist),
        // `/channel/<channel_id>`
        ["channel", channel_id] if !channel_id.is_empty() => {
            Some(UrlKind::Channel((*channel_id).to_string()))
        }
        // `/shorts/<video_id>`
        ["shorts", video_id] if !video_id.is_empty() => {
            Some(UrlKind::Short((*video_id).to_string()))
        }
        // `/<handle>`
        [path] if path.len() > 1 && path.starts_with('@') => {
            Some(UrlKind::ChannelHandle(path[1..].to_string()))
        }
        _ => None,
    }
}

/// Parses youtu.be URLs
fn parse_youtu_be_url(url: &Url) -> Option<UrlKind> {
    match path_segments(url)?.as_slice() {
        // `/video_id`
        [video_id] => Some(UrlKind::Video((*video_id).to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url::assert_parses;

    #[test]
    fn test_parse_video_urls() {
        assert_parses(parse_youtube_url, &[
            (
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
            (
                "https://youtube.com/watch?v=dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
            (
                "https://youtu.be/dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
            // A trailing slash does not corrupt the video id.
            (
                "https://youtu.be/dQw4w9WgXcQ/",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
        ]);
    }

    #[test]
    fn test_parse_shorts_urls() {
        assert_parses(parse_youtube_url, &[
            (
                "https://www.youtube.com/shorts/l4s8y-O_ols",
                Some(UrlKind::Short("l4s8y-O_ols".to_string())),
            ),
            (
                "https://youtube.com/shorts/l4s8y-O_ols/",
                Some(UrlKind::Short("l4s8y-O_ols".to_string())),
            ),
        ]);
    }

    #[test]
    fn test_parse_playlist_urls() {
        assert_parses(parse_youtube_url, &[(
            "https://www.youtube.com/playlist?list=PLF37D334894B07EEA",
            Some(UrlKind::Playlist("PLF37D334894B07EEA".to_string())),
        )]);
    }

    #[test]
    fn test_invalid_urls() {
        assert_parses(parse_youtube_url, &[
            ("https://example.com/watch?v=test", None),
            ("https://youtube.com/channel/", None),
            ("https://youtube.com/shorts/", None),
            ("https://youtu.be/", None),
            ("https://youtu.be/dQw4w9WgXcQ/subpage", None),
            // An empty handle is not a channel.
            ("https://youtube.com/@", None),
        ]);
    }

    #[test]
    fn it_should_parse_channel_urls() {
        assert_parses(parse_youtube_url, &[
            (
                "https://www.youtube.com/channel/UChuZAo1RKL85gev3Eal9_zg",
                Some(UrlKind::Channel("UChuZAo1RKL85gev3Eal9_zg".to_string())),
            ),
            (
                "https://www.youtube.com/channel/UChuZAo1RKL85gev3Eal9_zg/",
                Some(UrlKind::Channel("UChuZAo1RKL85gev3Eal9_zg".to_string())),
            ),
            (
                "https://www.youtube.com/@BreakingTaps",
                Some(UrlKind::ChannelHandle("BreakingTaps".to_string())),
            ),
            (
                "https://www.youtube.com/@BreakingTaps/",
                Some(UrlKind::ChannelHandle("BreakingTaps".to_string())),
            ),
        ]);
    }

    #[test]
    fn test_empty_query_ids_are_kept_verbatim() {
        // Unlike `/channel/` and `/shorts/`, which reject empty ids, an empty query id is
        // passed through as-is — pinned here so a change to that asymmetry is conscious.
        assert_parses(parse_youtube_url, &[
            ("https://youtube.com/watch?v=", Some(UrlKind::Video(String::new()))),
            (
                "https://youtube.com/playlist?list=",
                Some(UrlKind::Playlist(String::new())),
            ),
            // A missing id parses to nothing at all.
            ("https://youtube.com/watch", None),
            ("https://youtube.com/playlist", None),
        ]);
    }
}
