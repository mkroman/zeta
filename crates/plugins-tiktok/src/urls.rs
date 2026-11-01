//! Parsing and construction of TikTok URLs.

use url::Url;

use zeta_url::{is_identifier, is_numeric_segment, path_segments};

/// The hostname used for shortened URLs.
const TIKTOK_SHORT_HOST: &str = "vm.tiktok.com";

/// The standard hostname.
const TIKTOK_STANDARD_HOST: &str = "tiktok.com";

/// The www-prefixed standard hostname.
const TIKTOK_WWW_HOST: &str = "www.tiktok.com";

/// The TikTok hosts whose links this plugin handles.
pub const URL_HOSTS: &[&str] = &[TIKTOK_STANDARD_HOST, TIKTOK_SHORT_HOST, TIKTOK_WWW_HOST];

/// A TikTok link, identified by what it points at.
#[derive(Eq, PartialEq, Debug)]
pub enum TikTokLink {
    /// A video link, identified by its channel slug and video id.
    Video {
        /// The channel slug, e.g. `@dailymail`, or the anonymous bare `@` that short links
        /// redirect to when they do not carry the author.
        channel: String,
        /// The video id.
        id: String,
    },
    /// A channel profile link.
    Channel(String),
    /// A shortened share link that has to be resolved before the video can be identified.
    Shortened(String),
}

/// Returns the canonical URL for the given video.
#[must_use]
pub fn video_url(channel: &str, video_id: &str) -> String {
    format!("https://www.tiktok.com/{channel}/video/{video_id}")
}

/// Returns the short URL for the given id, used when resolving redirects.
#[must_use]
pub fn short_url(id: &str) -> String {
    format!("https://{TIKTOK_SHORT_HOST}/{id}/")
}

/// Parses the given `url` into a [`TikTokLink`], returning `None` if it isn't a TikTok link or
/// contains invalid components.
#[must_use]
pub fn parse_tiktok_url(url: &Url) -> Option<TikTokLink> {
    match url.host_str()? {
        TIKTOK_STANDARD_HOST | TIKTOK_WWW_HOST => parse_tiktok_com_url(url),
        TIKTOK_SHORT_HOST => parse_shortened_tiktok_url(url),
        _ => None,
    }
}

/// Parses vm.tiktok.com URLs.
fn parse_shortened_tiktok_url(url: &Url) -> Option<TikTokLink> {
    match path_segments(url)?.as_slice() {
        [id] if is_valid_short_id(id) => Some(TikTokLink::Shortened((*id).to_string())),
        _ => None,
    }
}

/// Parses tiktok.com URLs
fn parse_tiktok_com_url(url: &Url) -> Option<TikTokLink> {
    match path_segments(url)?.as_slice() {
        // `/@somechannel`
        [channel] if is_valid_channel_slug(channel) => {
            Some(TikTokLink::Channel((*channel).to_string()))
        }
        // `/@somechannel/video/7551110927479754006`, or `/@/video/7551110927479754006` — the
        // anonymous form short links redirect to when they do not carry the author.
        [channel, "video", video_id]
            if is_valid_video_channel_slug(channel) && is_numeric_segment(video_id) =>
        {
            Some(TikTokLink::Video {
                channel: (*channel).to_string(),
                id: (*video_id).to_string(),
            })
        }
        _ => None,
    }
}

/// Returns whether the segment is a valid channel slug (`@` followed by username characters).
#[must_use]
fn is_valid_channel_slug(segment: &str) -> bool {
    segment.starts_with('@') && is_identifier(&segment[1..], "._-")
}

/// Returns whether the segment is a valid channel slug in a video URL: a channel slug, or the
/// anonymous bare `@`, which the short links redirect to when they do not carry the author.
#[must_use]
fn is_valid_video_channel_slug(segment: &str) -> bool {
    segment == "@" || is_valid_channel_slug(segment)
}

/// Returns whether the segment is a valid shortened-link id.
#[must_use]
fn is_valid_short_id(id: &str) -> bool {
    is_identifier(id, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::assert_parses;

    #[test]
    fn test_parse_urls() {
        let video = |channel: &str, id: &str| {
            Some(TikTokLink::Video {
                channel: channel.to_string(),
                id: id.to_string(),
            })
        };

        assert_parses(
            parse_tiktok_url,
            &[
                // Channels.
                (
                    "https://www.tiktok.com/@dailymail",
                    Some(TikTokLink::Channel("@dailymail".to_string())),
                ),
                (
                    "https://tiktok.com/@user123",
                    Some(TikTokLink::Channel("@user123".to_string())),
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
                // The anonymous video form that short links redirect to when they do not carry
                // the author.
                (
                    "https://www.tiktok.com/@/video/7551042261212056854",
                    video("@", "7551042261212056854"),
                ),
                (
                    "https://www.tiktok.com/@/video/7551042261212056854/",
                    video("@", "7551042261212056854"),
                ),
                // Shortened links.
                (
                    "https://vm.tiktok.com/ZNdgoKow7/",
                    Some(TikTokLink::Shortened("ZNdgoKow7".to_string())),
                ),
                (
                    "https://vm.tiktok.com/ZNdgoKow7",
                    Some(TikTokLink::Shortened("ZNdgoKow7".to_string())),
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
            ],
        );
    }

    #[test]
    fn test_url_construction() {
        assert_eq!(
            video_url("@dailymail", "7541501431543532814"),
            "https://www.tiktok.com/@dailymail/video/7541501431543532814"
        );
        assert_eq!(short_url("ZNdgoKow7"), "https://vm.tiktok.com/ZNdgoKow7/");

        // Constructed URLs round-trip through the parser.
        let url = Url::parse(&video_url("@dailymail", "7541501431543532814")).unwrap();
        assert_eq!(
            parse_tiktok_url(&url),
            Some(TikTokLink::Video {
                channel: "@dailymail".to_string(),
                id: "7541501431543532814".to_string(),
            })
        );

        // The anonymous slug round-trips too.
        let url = Url::parse(&video_url("@", "7551042261212056854")).unwrap();
        assert_eq!(
            parse_tiktok_url(&url),
            Some(TikTokLink::Video {
                channel: "@".to_string(),
                id: "7551042261212056854".to_string(),
            })
        );
    }
}
