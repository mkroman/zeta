//! Host and path-shape classification of Instagram links.
//!
//! [`parse_instagram_url`] maps a URL onto an [`InstagramLink`], or `None` when the link is not
//! one this plugin handles. [`URL_HOSTS`] lists the hosts the plugin registers for, and the path
//! validators below define the shapes Instagram serves.

use url::Url;

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
pub(crate) const URL_HOSTS: &[&str] = &[
    INSTAGRAM_COM,
    INSTAGRAM_AM,
    L_INSTAGRAM_COM,
    M_INSTAGRAM_COM,
    WWW_INSTAGRAM_COM,
];

/// Path segments that name site resources rather than user profiles.
const RESERVED_SEGMENTS: &[&str] = &[
    "accounts", "direct", "explore", "p", "reel", "reels", "share", "stories", "tv",
];

/// An Instagram link, identified by what it points at.
#[derive(Eq, PartialEq, Debug)]
pub(crate) enum InstagramLink {
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
pub(crate) enum MediaKind {
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
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Post => "post",
            Self::Reel => "reel",
            Self::Tv => "video",
        }
    }

    /// The URL path segment identifying the kind of media in canonical URLs.
    #[must_use]
    pub(crate) const fn segment(self) -> &'static str {
        match self {
            Self::Post => "p",
            Self::Reel => "reel",
            Self::Tv => "tv",
        }
    }

    /// Returns the media kind named by the given URL path segment, if any.
    pub(super) fn from_segment(segment: &str) -> Option<Self> {
        match segment {
            "p" => Some(Self::Post),
            "reel" | "reels" => Some(Self::Reel),
            "tv" => Some(Self::Tv),
            _ => None,
        }
    }
}

/// Parses the given `url` into an [`InstagramLink`], returning `None` if it isn't an Instagram
/// link or contains invalid components.
#[must_use]
pub(crate) fn parse_instagram_url(url: &Url) -> Option<InstagramLink> {
    let host = url.host_str()?;

    // `instagr.am` serves the same path structure as the standard hosts, but links that don't
    // parse as known resources are share links that have to be resolved through a redirect.
    let host = zeta_url::strip_www(host);

    match host {
        INSTAGRAM_COM | M_INSTAGRAM_COM => parse_instagram_com_url(url),
        INSTAGRAM_AM => {
            parse_instagram_com_url(url).or_else(|| Some(InstagramLink::Shortened(url.clone())))
        }
        // Link wrappers point at the target through their `u` query parameter.
        L_INSTAGRAM_COM => {
            let target = zeta_url::query_param(url, "u")?;

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
    match zeta_url::path_segments(url)?.as_slice() {
        // `/share/<token>` — app share links, resolved through a redirect.
        ["share", token] if is_valid_shortcode(token) => {
            Some(InstagramLink::Shortened(url.clone()))
        }
        // `/stories/<user>/<id>` — a story or highlight.
        ["stories", username, id]
            if is_valid_username(username) && zeta_url::is_numeric_segment(id) =>
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
    segment.len() <= 30 && zeta_url::is_identifier(segment, "._")
}

/// Returns whether the segment is a valid media shortcode: base64url characters, as assigned by
/// Instagram.
#[must_use]
fn is_valid_shortcode(segment: &str) -> bool {
    zeta_url::is_identifier(segment, "_-")
}
