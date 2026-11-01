//! Parsing of YouTube URLs, shared by the `youtube` and `ofn` plugins.

use std::borrow::Cow;

use url::Url;

use zeta_url::{path_segments, query_param};

/// The hostname of shortened YouTube URLs.
const YOUTU_BE_HOST: &str = "youtu.be";

/// The YouTube.com hostname.
const YOUTUBE_COM_HOST: &str = "youtube.com";

/// The www-prefixed YouTube.com hostname.
const YOUTUBE_COM_WWW_HOST: &str = "www.youtube.com";

/// The YouTube hosts whose links the `youtube` plugin handles.
pub const URL_HOSTS: &[&str] = &[YOUTU_BE_HOST, YOUTUBE_COM_HOST, YOUTUBE_COM_WWW_HOST];

/// The kind of YouTube resource a URL points at.
///
/// The ids borrow from the URL they were parsed from; only a percent-encoded query value needs
/// its own allocation.
#[derive(Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum UrlKind<'a> {
    /// Direct link to a video (e.g., `youtube.com/watch?v=VIDEO_ID` or `youtu.be/VIDEO_ID`)
    Video(Cow<'a, str>),
    /// Link to a short video (e.g., `youtube.com/shorts/VIDEO_ID`)
    Short(Cow<'a, str>),
    /// Direct link to a channel using channel ID (e.g., `youtube.com/channel/CHANNEL_ID`)
    Channel(Cow<'a, str>),
    /// Link to a channel using the @ handle (e.g., `youtube.com/@ChannelName`)
    ChannelHandle(Cow<'a, str>),
    /// Direct link to a playlist (e.g., `youtube.com/playlist?list=PLAYLIST_ID`)
    Playlist(Cow<'a, str>),
}

/// Parses the given `url` and returns a [`UrlKind`] depending on the type of YouTube URL.
#[must_use]
pub fn parse_youtube_url(url: &Url) -> Option<UrlKind<'_>> {
    match url.host_str()? {
        YOUTU_BE_HOST => parse_youtu_be_url(url),
        YOUTUBE_COM_HOST | YOUTUBE_COM_WWW_HOST => parse_youtube_com_url(url),
        _ => None,
    }
}

/// Parses youtube.com URLs
fn parse_youtube_com_url(url: &Url) -> Option<UrlKind<'_>> {
    match path_segments(url)?.as_slice() {
        // `/watch?v=<video_id>`
        ["watch"] => query_param(url, "v").map(UrlKind::Video),
        // `/playlist?list=<playlist_id>`
        ["playlist"] => query_param(url, "list").map(UrlKind::Playlist),
        // `/channel/<channel_id>`
        ["channel", channel_id] if !channel_id.is_empty() => {
            Some(UrlKind::Channel(Cow::Borrowed(channel_id)))
        }
        // `/shorts/<video_id>`
        ["shorts", video_id] if !video_id.is_empty() => {
            Some(UrlKind::Short(Cow::Borrowed(video_id)))
        }
        // `/<handle>`
        [path] if path.len() > 1 && path.starts_with('@') => {
            Some(UrlKind::ChannelHandle(Cow::Borrowed(&path[1..])))
        }
        _ => None,
    }
}

/// Parses youtu.be URLs
fn parse_youtu_be_url(url: &Url) -> Option<UrlKind<'_>> {
    match path_segments(url)?.as_slice() {
        // `/video_id`
        [video_id] => Some(UrlKind::Video(Cow::Borrowed(video_id))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::assert_parses;

    impl UrlKind<'_> {
        /// Copies the ids out of the parsed URL, so a test can name the expected value before
        /// the URL it borrows from exists.
        fn into_owned(self) -> UrlKind<'static> {
            match self {
                Self::Video(id) => UrlKind::Video(Cow::Owned(id.into_owned())),
                Self::Short(id) => UrlKind::Short(Cow::Owned(id.into_owned())),
                Self::Channel(id) => UrlKind::Channel(Cow::Owned(id.into_owned())),
                Self::ChannelHandle(id) => UrlKind::ChannelHandle(Cow::Owned(id.into_owned())),
                Self::Playlist(id) => UrlKind::Playlist(Cow::Owned(id.into_owned())),
            }
        }
    }

    /// Asserts the parsed kind against owned expectations, which `assert_parses` can build
    /// before it parses the URL.
    fn assert_parses_youtube(cases: &[(&str, Option<UrlKind<'static>>)]) {
        assert_parses(|url| parse_youtube_url(url).map(UrlKind::into_owned), cases);
    }

    #[test]
    fn test_parse_video_urls() {
        assert_parses_youtube(&[
            (
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".into())),
            ),
            (
                "https://youtube.com/watch?v=dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".into())),
            ),
            (
                "https://youtu.be/dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".into())),
            ),
            // A trailing slash does not corrupt the video id.
            (
                "https://youtu.be/dQw4w9WgXcQ/",
                Some(UrlKind::Video("dQw4w9WgXcQ".into())),
            ),
        ]);
    }

    #[test]
    fn test_parse_shorts_urls() {
        assert_parses_youtube(&[
            (
                "https://www.youtube.com/shorts/l4s8y-O_ols",
                Some(UrlKind::Short("l4s8y-O_ols".into())),
            ),
            (
                "https://youtube.com/shorts/l4s8y-O_ols/",
                Some(UrlKind::Short("l4s8y-O_ols".into())),
            ),
        ]);
    }

    #[test]
    fn test_parse_playlist_urls() {
        assert_parses_youtube(&[(
            "https://www.youtube.com/playlist?list=PLF37D334894B07EEA",
            Some(UrlKind::Playlist("PLF37D334894B07EEA".into())),
        )]);
    }

    #[test]
    fn test_invalid_urls() {
        assert_parses_youtube(&[
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
        assert_parses_youtube(&[
            (
                "https://www.youtube.com/channel/UChuZAo1RKL85gev3Eal9_zg",
                Some(UrlKind::Channel("UChuZAo1RKL85gev3Eal9_zg".into())),
            ),
            (
                "https://www.youtube.com/channel/UChuZAo1RKL85gev3Eal9_zg/",
                Some(UrlKind::Channel("UChuZAo1RKL85gev3Eal9_zg".into())),
            ),
            (
                "https://www.youtube.com/@BreakingTaps",
                Some(UrlKind::ChannelHandle("BreakingTaps".into())),
            ),
            (
                "https://www.youtube.com/@BreakingTaps/",
                Some(UrlKind::ChannelHandle("BreakingTaps".into())),
            ),
        ]);
    }

    #[test]
    fn test_empty_query_ids_are_kept_verbatim() {
        // Unlike `/channel/` and `/shorts/`, which reject empty ids, an empty query id is
        // passed through as-is — pinned here so a change to that asymmetry is conscious.
        assert_parses_youtube(&[
            (
                "https://youtube.com/watch?v=",
                Some(UrlKind::Video(String::new().into())),
            ),
            (
                "https://youtube.com/playlist?list=",
                Some(UrlKind::Playlist(String::new().into())),
            ),
            // A missing id parses to nothing at all.
            ("https://youtube.com/watch", None),
            ("https://youtube.com/playlist", None),
        ]);
    }
}
