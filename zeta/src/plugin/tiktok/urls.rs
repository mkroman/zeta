//! Parsing and classification of TikTok URLs.

use url::Url;

/// The hostname used for shortened URLs.
const TIKTOK_SHORT_HOST: &str = "vm.tiktok.com";

/// The standard hostname.
const TIKTOK_STANDARD_HOST: &str = "tiktok.com";

/// The kind of TikTok URL.
#[derive(Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum UrlKind {
    /// A video URL, consisting of the channel slug and the video id.
    Video(String, String),
    /// A channel URL.
    Channel(String),
    /// A shortened video URL.
    Shortened(String),
}

/// Parses the given `url` and returns a [`UrlKind`] depending on the type of TikTok URL.
#[must_use]
pub fn classify_tiktok_url(url: &Url) -> Option<UrlKind> {
    match url.host_str()? {
        TIKTOK_STANDARD_HOST | "www.tiktok.com" => parse_tiktok_com_url(url),
        TIKTOK_SHORT_HOST => parse_shortened_tiktok_url(url),
        _ => None,
    }
}

fn parse_shortened_tiktok_url(url: &Url) -> Option<UrlKind> {
    let segments: Vec<&str> = url.path_segments()?.collect();

    match segments.as_slice() {
        [id] | [id, ""] if is_valid_short_id(id) => Some(UrlKind::Shortened((*id).to_string())),
        _ => None,
    }
}

/// Parses tiktok.com URLs
fn parse_tiktok_com_url(url: &Url) -> Option<UrlKind> {
    let segments: Vec<&str> = url.path_segments()?.collect();

    match segments.as_slice() {
        // `/@somechannel`
        [channel] if is_valid_channel_slug(channel) => {
            Some(UrlKind::Channel((*channel).to_string()))
        }
        // `/@somechannel/video/7551110927479754006`
        [channel, "video", video_id] | [channel, "video", video_id, ""]
            if is_valid_channel_slug(channel) && is_valid_video_id(video_id) =>
        {
            Some(UrlKind::Video(
                (*channel).to_string(),
                (*video_id).to_string(),
            ))
        }
        _ => None,
    }
}

/// Returns whether the segment is a valid channel slug (`@` followed by username characters).
///
/// Path segments are percent-decoded, so this guards against characters that could alter the
/// meaning of canonical URLs built from these segments.
#[must_use]
fn is_valid_channel_slug(segment: &str) -> bool {
    segment.len() > 1
        && segment.starts_with('@')
        && segment[1..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Returns whether the segment is a valid video id (TikTok video ids are numeric).
#[must_use]
fn is_valid_video_id(video_id: &str) -> bool {
    !video_id.is_empty() && video_id.chars().all(|c| c.is_ascii_digit())
}

/// Returns whether the segment is a valid shortened-link id.
#[must_use]
fn is_valid_short_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_urls() {
        let video =
            |channel: &str, id: &str| Some(UrlKind::Video(channel.to_string(), id.to_string()));

        let test_cases = [
            // Channels.
            (
                "https://www.tiktok.com/@dailymail",
                Some(UrlKind::Channel("@dailymail".to_string())),
            ),
            (
                "https://tiktok.com/@user123",
                Some(UrlKind::Channel("@user123".to_string())),
            ),
            // Videos.
            (
                "https://www.tiktok.com/@dailymail/video/7541501431543532814",
                video("@dailymail", "7541501431543532814"),
            ),
            (
                "https://www.tiktok.com/@dailymail/video/7541501431543532814/",
                video("@dailymail", "7541501431543532814"),
            ),
            (
                "https://www.tiktok.com/@user.name_1-x/video/7541501431543532814",
                video("@user.name_1-x", "7541501431543532814"),
            ),
            // Shortened links.
            (
                "https://vm.tiktok.com/ZNdgoKow7/",
                Some(UrlKind::Shortened("ZNdgoKow7".to_string())),
            ),
            (
                "https://vm.tiktok.com/ZNdgoKow7",
                Some(UrlKind::Shortened("ZNdgoKow7".to_string())),
            ),
            // Invalid URLs.
            ("https://www.tiktok.com/@", None),
            ("https://www.tiktok.com/@dailymail/video/", None),
            ("https://vm.tiktok.com/", None),
            // Percent-encoded path traversal in the components is rejected.
            ("https://www.tiktok.com/@user/video/123%2F..%2Fother", None),
            ("https://vm.tiktok.com/abc%2F..", None),
            // Video ids are numeric.
            ("https://www.tiktok.com/@user/video/abc123", None),
            // Channel slugs and short ids are limited to username/identifier characters.
            ("https://www.tiktok.com/@user;rm/video/123", None),
            ("https://vm.tiktok.com/ab-cd/", None),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(classify_tiktok_url(&url), expected, "for {url_str}");
        }
    }
}
