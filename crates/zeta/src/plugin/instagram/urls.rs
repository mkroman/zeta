//! Parsing and construction of Instagram URLs.

use url::Url;

use crate::url::{is_identifier, is_numeric_segment, path_segments, query_param};

/// The standard hostname.
const INSTAGRAM_COM: &str = "instagram.com";

/// The www-prefixed standard hostname.
const WWW_INSTAGRAM_COM: &str = "www.instagram.com";

/// The mobile-prefixed standard hostname.
const M_INSTAGRAM_COM: &str = "m.instagram.com";

/// The short hostname.
const INSTAGRAM_AM: &str = "instagr.am";

/// The hostname wrapping links shared from Instagram, pointing at the target through its `u`
/// query parameter.
const L_INSTAGRAM_COM: &str = "l.instagram.com";

/// The Instagram hosts whose links this plugin handles.
pub(super) const URL_HOSTS: &[&str] = &[
    INSTAGRAM_COM,
    INSTAGRAM_AM,
    L_INSTAGRAM_COM,
    M_INSTAGRAM_COM,
    WWW_INSTAGRAM_COM,
];

/// Path segments that name site resources rather than user profiles.
const RESERVED_SEGMENTS: &[&str] = &["accounts", "direct", "explore", "p", "reel", "reels", "share", "stories", "tv"];

/// An Instagram link, identified by what it points at.
#[derive(Eq, PartialEq, Debug)]
pub enum InstagramLink {
    /// A media post — a feed post, reel, or IGTV video — identified by its shortcode.
    Media {
        /// The kind of media the link points at.
        kind: MediaKind,
        /// The media shortcode.
        id: String,
    },
    /// A story or highlight link, identified by its author and media id.
    Story {
        /// The username that published the story, or `highlights` for highlight links.
        username: String,
        /// The story media id.
        id: String,
    },
    /// A user profile link.
    Profile(String),
    /// A share or short-domain link that has to be resolved through a redirect before the
    /// resource it points at can be identified.
    Shortened(Url),
}

/// The kind of media an Instagram link points at.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum MediaKind {
    /// A standard feed post: photos, videos, or a carousel.
    Post,
    /// A short-form vertical video.
    Reel,
    /// A legacy IGTV long-form video.
    Tv,
}

impl MediaKind {
    /// The word used for the kind of media in summaries.
    #[must_use]
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Post => "post",
            Self::Reel => "reel",
            Self::Tv => "video",
        }
    }

    /// The URL path segment identifying the kind of media in canonical URLs.
    #[must_use]
    pub(super) const fn segment(self) -> &'static str {
        match self {
            Self::Post => "p",
            Self::Reel => "reel",
            Self::Tv => "tv",
        }
    }

    /// Returns the media kind named by the given URL path segment, if any.
    fn from_segment(segment: &str) -> Option<Self> {
        match segment {
            "p" => Some(Self::Post),
            "reel" | "reels" => Some(Self::Reel),
            "tv" => Some(Self::Tv),
            _ => None,
        }
    }
}

/// Returns the canonical URL for the given media.
///
/// The canonical URL is built on the standard host; the mirror and metadata requests always use
/// it, no matter which host the link was posted with.
#[must_use]
pub fn media_url(kind: MediaKind, id: &str) -> String {
    format!("https://www.instagram.com/{}/{}", kind.segment(), id)
}

/// Returns the canonical URL for the given story.
#[must_use]
pub fn story_url(username: &str, id: &str) -> String {
    format!("https://www.instagram.com/stories/{username}/{id}")
}

/// Parses the given `url` into an [`InstagramLink`], returning `None` if it isn't an Instagram
/// link or contains invalid components.
#[must_use]
pub fn parse_instagram_url(url: &Url) -> Option<InstagramLink> {
    let host = url.host_str()?;

    // `instagr.am` serves the same path structure as the standard hosts, but links that don't
    // parse as known resources are share links that have to be resolved through a redirect.
    let host = host.strip_prefix("www.").unwrap_or(host);

    match host {
        INSTAGRAM_COM | M_INSTAGRAM_COM => parse_instagram_com_url(url),
        INSTAGRAM_AM => {
            parse_instagram_com_url(url).or_else(|| Some(InstagramLink::Shortened(url.clone())))
        }
        // Link wrappers point at the target through their `u` query parameter.
        L_INSTAGRAM_COM => {
            let target = query_param(url, "u")?;

            let target = Url::parse(&target).ok()?;
            if target.host_str() == Some(L_INSTAGRAM_COM) {
                return None;
            }

            parse_instagram_url(&target)
        }
        _ => None,
    }
}

/// Parses the path structure shared by the standard Instagram hosts.
fn parse_instagram_com_url(url: &Url) -> Option<InstagramLink> {
    match path_segments(url)?.as_slice() {
        // `/share/<token>` — app share links, resolved through a redirect.
        ["share", token] if is_valid_shortcode(token) => {
            Some(InstagramLink::Shortened(url.clone()))
        }
        // `/stories/<user>/<id>` — a story or highlight.
        ["stories", username, id]
            if is_valid_username(username) && is_numeric_segment(id) =>
        {
            Some(InstagramLink::Story {
                username: (*username).to_string(),
                id: (*id).to_string(),
            })
        }
        // `/p/<id>`, `/reel/<id>`, `/reels/<id>`, or `/tv/<id>`, optionally followed by an `embed`
        // or `embed/captioned` suffix — with an optional user prefix in front.
        [segment, id, rest @ ..] if is_valid_media(segment, id, rest) => {
            Some(InstagramLink::Media {
                kind: MediaKind::from_segment(segment)?,
                id: (*id).to_string(),
            })
        }
        [username, segment, id, rest @ ..]
            if is_valid_username(username) && is_valid_media(segment, id, rest) =>
        {
            Some(InstagramLink::Media {
                kind: MediaKind::from_segment(segment)?,
                id: (*id).to_string(),
            })
        }
        // `/<username>` — a user profile link.
        [username] if is_valid_username(username) && !RESERVED_SEGMENTS.contains(username) => {
            Some(InstagramLink::Profile((*username).to_string()))
        }
        _ => None,
    }
}

/// Returns whether `segment`, `id`, and the following segments form a media path: one of the
/// media segments with a valid shortcode, optionally followed by an ignorable embed suffix.
#[must_use]
fn is_valid_media(segment: &str, id: &str, rest: &[&str]) -> bool {
    MediaKind::from_segment(segment).is_some() && is_valid_shortcode(id) && is_embed_suffix(rest)
}

/// Returns whether the segments following a media path in a URL are an ignorable embed suffix.
#[must_use]
fn is_embed_suffix(segments: &[&str]) -> bool {
    matches!(segments, [] | ["embed"] | ["embed", "captioned"])
}

/// Returns whether the segment is a valid username: 1-30 ASCII alphanumerics, periods, or
/// underscores, as accepted by Instagram.
#[must_use]
fn is_valid_username(segment: &str) -> bool {
    segment.len() <= 30 && is_identifier(segment, "._")
}

/// Returns whether the segment is a valid media shortcode: base64url characters, as assigned by
/// Instagram.
#[must_use]
fn is_valid_shortcode(segment: &str) -> bool {
    is_identifier(segment, "_-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::assert_parses;

    #[test]
    fn test_parse_valid_urls() {
        let media = |kind: MediaKind, id: &str| {
            Some(InstagramLink::Media {
                kind,
                id: id.to_string(),
            })
        };
        let story = |username: &str, id: &str| {
            Some(InstagramLink::Story {
                username: username.to_string(),
                id: id.to_string(),
            })
        };
        let shortened = |url: &str| {
            Some(InstagramLink::Shortened(Url::parse(url).unwrap()))
        };

        assert_parses(parse_instagram_url, &[
            // Profiles.
            ("https://www.instagram.com/nike/", Some(InstagramLink::Profile("nike".to_string()))),
            ("https://www.instagram.com/nike", Some(InstagramLink::Profile("nike".to_string()))),
            (
                "https://www.instagram.com/user.name_1/",
                Some(InstagramLink::Profile("user.name_1".to_string())),
            ),
            // Media.
            ("https://www.instagram.com/p/DdUGmgAifcq", media(MediaKind::Post, "DdUGmgAifcq")),
            ("https://www.instagram.com/p/C7_Hlo8y9aP/", media(MediaKind::Post, "C7_Hlo8y9aP")),
            ("https://www.instagram.com/p/C7_Hlo8y9aP", media(MediaKind::Post, "C7_Hlo8y9aP")),
            ("https://instagram.com/p/C7_Hlo8y9aP/", media(MediaKind::Post, "C7_Hlo8y9aP")),
            ("https://m.instagram.com/p/C7_Hlo8y9aP/", media(MediaKind::Post, "C7_Hlo8y9aP")),
            ("https://www.instagram.com/reel/Chunk8-jurw/", media(MediaKind::Reel, "Chunk8-jurw")),
            ("https://www.instagram.com/reels/Cop84x6u7CP/", media(MediaKind::Reel, "Cop84x6u7CP")),
            ("https://www.instagram.com/tv/BkfuX9UB-eK/", media(MediaKind::Tv, "BkfuX9UB-eK")),
            (
                "https://www.instagram.com/marvelskies.fc/reel/CWqAgUZgCku/",
                media(MediaKind::Reel, "CWqAgUZgCku"),
            ),
            // Embed suffixes are tolerated.
            (
                "https://www.instagram.com/p/C7_Hlo8y9aP/embed/",
                media(MediaKind::Post, "C7_Hlo8y9aP"),
            ),
            (
                "https://www.instagram.com/p/C7_Hlo8y9aP/embed/captioned/",
                media(MediaKind::Post, "C7_Hlo8y9aP"),
            ),
            // Stories and highlights.
            (
                "https://www.instagram.com/stories/fruits_zipper/3570766765028588805/",
                story("fruits_zipper", "3570766765028588805"),
            ),
            (
                "https://www.instagram.com/stories/fruits_zipper/3570766765028588805",
                story("fruits_zipper", "3570766765028588805"),
            ),
            (
                "https://www.instagram.com/stories/highlights/18090946048123978/",
                story("highlights", "18090946048123978"),
            ),
            // Share links.
            (
                "https://www.instagram.com/share/AbCdEf1/?igsh=track",
                shortened("https://www.instagram.com/share/AbCdEf1/?igsh=track"),
            ),
            ("https://www.instagram.com/share/AbCdEf1/", shortened("https://www.instagram.com/share/AbCdEf1/")),
            // Short-domain links.
            ("https://instagr.am/p/C7_Hlo8y9aP/", media(MediaKind::Post, "C7_Hlo8y9aP")),
            ("https://instagr.am/nike/", Some(InstagramLink::Profile("nike".to_string()))),
            ("https://instagr.am/AbCdEf_", Some(InstagramLink::Profile("AbCdEf_".to_string()))),
            // Short-domain links that do not parse as a profile are resolved through a redirect.
            ("https://instagr.am/AbC-dEf", shortened("https://instagr.am/AbC-dEf")),
            // Link wrappers point at the target through their `u` query parameter.
            (
                "https://l.instagram.com/?u=https%3A%2F%2Fwww.instagram.com%2Fp%2FDdUGmgAifcq%2F&e=abc",
                media(MediaKind::Post, "DdUGmgAifcq"),
            ),
            (
                "https://l.instagram.com/?u=https%3A%2F%2Fwww.instagram.com%2Fuser.name%2F",
                Some(InstagramLink::Profile("user.name".to_string())),
            ),
            // Wrapped targets that are not Instagram links, missing, or nested wrappers are
            // ignored.
            ("https://l.instagram.com/?u=https%3A%2F%2Fexample.com%2F", None),
            ("https://l.instagram.com/?e=abc", None),
            ("https://l.instagram.com/?u=https%3A%2F%2Fl.instagram.com%2F%3Fu%3Dx",
                None,
            ),
        ]);
    }

    #[test]
    fn test_parse_invalid_urls() {
        assert_parses(parse_instagram_url, &[
            ("https://www.instagram.com/p/", None),
            ("https://www.instagram.com/p/C7_Hlo8y9aP/other", None),
            ("https://www.instagram.com/stories/fruits_zipper/", None),
            ("https://www.instagram.com/stories/", None),
            ("https://www.instagram.com/share/", None),
            // Percent-encoded path traversal in the components is rejected.
            ("https://www.instagram.com/p/C7_Hlo8y9aP%2F..%2Fother", None),
            ("https://www.instagram.com/p/C7_Hlo8y9aP..", None),
            // Shortcodes are limited to base64url characters.
            ("https://www.instagram.com/p/ab;cd", None),
            ("https://www.instagram.com/p/ab.cd", None),
            // Story ids are numeric.
            ("https://www.instagram.com/stories/user/abc123", None),
            // Usernames are limited to username characters.
            ("https://www.instagram.com/user;rm", None),
            ("https://www.instagram.com/user-name", None),
            // Usernames are limited to 30 characters.
            ("https://www.instagram.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/", None),
            // Site resources are not profiles.
            ("https://www.instagram.com/explore/tags/style/", None),
            ("https://www.instagram.com/accounts/login/", None),
            ("https://www.instagram.com/direct/t/12345/", None),
            ("https://www.instagram.com/share", None),
            // Reels audio links are not media links.
            ("https://www.instagram.com/reels/audio/Cop84x6u7CP/", None),
            // Other hosts are not Instagram links.
            ("https://www.example.com/p/C7_Hlo8y9aP/", None),
        ]);
    }

    #[test]
    fn test_url_construction() {
        assert_eq!(
            media_url(MediaKind::Post, "C7_Hlo8y9aP"),
            "https://www.instagram.com/p/C7_Hlo8y9aP"
        );
        assert_eq!(
            media_url(MediaKind::Reel, "Chunk8-jurw"),
            "https://www.instagram.com/reel/Chunk8-jurw"
        );
        assert_eq!(
            media_url(MediaKind::Tv, "BkfuX9UB-eK"),
            "https://www.instagram.com/tv/BkfuX9UB-eK"
        );
        assert_eq!(
            story_url("fruits_zipper", "3570766765028588805"),
            "https://www.instagram.com/stories/fruits_zipper/3570766765028588805"
        );

        // Constructed URLs round-trip through the parser.
        let url = Url::parse(&media_url(MediaKind::Reel, "Chunk8-jurw")).unwrap();
        assert_eq!(
            parse_instagram_url(&url),
            Some(InstagramLink::Media {
                kind: MediaKind::Reel,
                id: "Chunk8-jurw".to_string(),
            })
        );

        let url = Url::parse(&story_url("fruits_zipper", "3570766765028588805")).unwrap();
        assert_eq!(
            parse_instagram_url(&url),
            Some(InstagramLink::Story {
                username: "fruits_zipper".to_string(),
                id: "3570766765028588805".to_string(),
            })
        );
    }

    #[test]
    fn test_media_kind_labels_and_segments() {
        assert_eq!(MediaKind::Post.label(), "post");
        assert_eq!(MediaKind::Reel.label(), "reel");
        assert_eq!(MediaKind::Tv.label(), "video");

        assert_eq!(MediaKind::from_segment("p"), Some(MediaKind::Post));
        assert_eq!(MediaKind::from_segment("reels"), Some(MediaKind::Reel));
        assert_eq!(MediaKind::from_segment("reel"), Some(MediaKind::Reel));
        assert_eq!(MediaKind::from_segment("tv"), Some(MediaKind::Tv));
        assert_eq!(MediaKind::from_segment("stories"), None);
    }
}
