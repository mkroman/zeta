use url::Url;

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
pub(super) fn parse_youtube_url(url: &Url) -> Option<UrlKind> {
    match url.host_str()? {
        "youtu.be" => parse_youtu_be_url(url),
        "youtube.com" | "www.youtube.com" => parse_youtube_com_url(url),
        _ => None,
    }
}

/// Extracts a query parameter value from a URL
fn extract_query_param(url: &Url, param: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == param)
        .map(|(_, value)| value.to_string())
}

/// Parses youtube.com URLs
fn parse_youtube_com_url(url: &Url) -> Option<UrlKind> {
    let segments: Vec<&str> = url.path_segments()?.collect();

    match segments.as_slice() {
        // `/watch?v=<video_id>`
        ["watch"] => extract_query_param(url, "v").map(UrlKind::Video),
        // `/playlist?list=<playlist_id>`
        ["playlist"] => extract_query_param(url, "list").map(UrlKind::Playlist),
        // `/channel/<channel_id>`
        ["channel", channel_id] if !channel_id.is_empty() => {
            Some(UrlKind::Channel((*channel_id).to_string()))
        }
        ["shorts", video_id] if !video_id.is_empty() => {
            Some(UrlKind::Short((*video_id).to_string()))
        }
        // `/*`
        [path] if path.starts_with('@') && path.len() > 1 => {
            Some(UrlKind::ChannelHandle(path[1..].to_string()))
        }
        _ => None,
    }
}

/// Parses youtu.be URLs
fn parse_youtu_be_url(url: &Url) -> Option<UrlKind> {
    let path = url.path();

    if path.len() > 1 {
        return Some(UrlKind::Video(path[1..].to_owned()));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_youtube_com_video_urls() {
        let test_cases = [
            (
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
            (
                "https://youtube.com/watch?v=dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_youtube_url(&url), expected);
        }
    }

    #[test]
    fn test_parse_youtube_com_shorts_urls() {
        let test_cases = [
            (
                "https://www.youtube.com/shorts/l4s8y-O_ols",
                Some(UrlKind::Short("l4s8y-O_ols".to_string())),
            ),
            (
                "https://youtube.com/shorts/l4s8y-O_ols",
                Some(UrlKind::Short("l4s8y-O_ols".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_youtube_url(&url), expected);
        }
    }

    #[test]
    fn test_parse_youtu_be_video_urls() {
        let test_cases = [(
            "https://youtu.be/dQw4w9WgXcQ",
            Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
        )];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_youtube_url(&url), expected);
        }
    }

    #[test]
    fn test_parse_playlist_urls() {
        let test_cases = [(
            "https://www.youtube.com/playlist?list=PLF37D334894B07EEA",
            Some(UrlKind::Playlist("PLF37D334894B07EEA".to_string())),
        )];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_youtube_url(&url), expected);
        }
    }

    #[test]
    fn test_invalid_urls() {
        let invalid_urls = [
            "https://example.com/watch?v=test",
            "https://youtube.com/channel/",
            "https://youtu.be/",
        ];

        for url_str in invalid_urls {
            let url = Url::parse(url_str).unwrap();
            assert_eq!(parse_youtube_url(&url), None);
        }
    }

    #[test]
    fn it_should_parse_channel_urls() {
        let test_cases = [
            (
                "https://www.youtube.com/channel/UChuZAo1RKL85gev3Eal9_zg",
                Some(UrlKind::Channel("UChuZAo1RKL85gev3Eal9_zg".to_string())),
            ),
            (
                "https://www.youtube.com/@BreakingTaps",
                Some(UrlKind::ChannelHandle("BreakingTaps".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_youtube_url(&url), expected);
        }
    }
}
