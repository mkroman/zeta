//! Tests for URL classification and canonical URL construction.

use super::*;

use url::Url;
use zeta_test_support::{assert_parses, assert_url_hosts_cover};

#[test]
// The table-driven cases below are re-wrapped by rustfmt, which stretches the function
// past the line-count lint; the coverage is worth more than the split.
#[allow(clippy::too_many_lines)]
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
    let shortened = |url: &str| Some(InstagramLink::Shortened(Url::parse(url).unwrap()));

    assert_parses(
        parse_instagram_url,
        &[
            // Profiles.
            (
                "https://www.instagram.com/nike/",
                Some(InstagramLink::Profile("nike".to_string())),
            ),
            (
                "https://www.instagram.com/nike",
                Some(InstagramLink::Profile("nike".to_string())),
            ),
            (
                "https://www.instagram.com/user.name_1/",
                Some(InstagramLink::Profile("user.name_1".to_string())),
            ),
            // Media.
            (
                "https://www.instagram.com/p/DdUGmgAifcq",
                media(MediaKind::Post, "DdUGmgAifcq"),
            ),
            (
                "https://www.instagram.com/p/C7_Hlo8y9aP/",
                media(MediaKind::Post, "C7_Hlo8y9aP"),
            ),
            (
                "https://www.instagram.com/p/C7_Hlo8y9aP",
                media(MediaKind::Post, "C7_Hlo8y9aP"),
            ),
            (
                "https://instagram.com/p/C7_Hlo8y9aP/",
                media(MediaKind::Post, "C7_Hlo8y9aP"),
            ),
            (
                "https://m.instagram.com/p/C7_Hlo8y9aP/",
                media(MediaKind::Post, "C7_Hlo8y9aP"),
            ),
            (
                "https://www.instagram.com/reel/Chunk8-jurw/",
                media(MediaKind::Reel, "Chunk8-jurw"),
            ),
            (
                "https://www.instagram.com/reels/Cop84x6u7CP/",
                media(MediaKind::Reel, "Cop84x6u7CP"),
            ),
            (
                "https://www.instagram.com/tv/BkfuX9UB-eK/",
                media(MediaKind::Tv, "BkfuX9UB-eK"),
            ),
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
            (
                "https://www.instagram.com/share/AbCdEf1/",
                shortened("https://www.instagram.com/share/AbCdEf1/"),
            ),
            // Short-domain links.
            (
                "https://instagr.am/p/C7_Hlo8y9aP/",
                media(MediaKind::Post, "C7_Hlo8y9aP"),
            ),
            (
                "https://instagr.am/nike/",
                Some(InstagramLink::Profile("nike".to_string())),
            ),
            (
                "https://instagr.am/AbCdEf_",
                Some(InstagramLink::Profile("AbCdEf_".to_string())),
            ),
            // Short-domain links that do not parse as a profile are resolved through a redirect.
            (
                "https://instagr.am/AbC-dEf",
                shortened("https://instagr.am/AbC-dEf"),
            ),
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
            (
                "https://l.instagram.com/?u=https%3A%2F%2Fexample.com%2F",
                None,
            ),
            ("https://l.instagram.com/?e=abc", None),
            (
                "https://l.instagram.com/?u=https%3A%2F%2Fl.instagram.com%2F%3Fu%3Dx",
                None,
            ),
        ],
    );
}

#[test]
fn test_parse_invalid_urls() {
    assert_parses(
        parse_instagram_url,
        &[
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
            (
                "https://www.instagram.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
                None,
            ),
            // Site resources are not profiles.
            ("https://www.instagram.com/explore/tags/style/", None),
            ("https://www.instagram.com/accounts/login/", None),
            ("https://www.instagram.com/direct/t/12345/", None),
            ("https://www.instagram.com/share", None),
            // Reels audio links are not media links.
            ("https://www.instagram.com/reels/audio/Cop84x6u7CP/", None),
            // Other hosts are not Instagram links.
            ("https://www.example.com/p/C7_Hlo8y9aP/", None),
        ],
    );
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

/// Every advertised host has exactly one case, and the parser accepts it.
#[test]
fn url_hosts_cover_the_classified_hosts() {
    let cases = [
        ("instagram.com", "/p/DdUGmgAifcq"),
        ("instagr.am", "/nike/"),
        (
            "l.instagram.com",
            "/?u=https%3A%2F%2Fwww.instagram.com%2Fp%2FDdUGmgAifcq%2F",
        ),
        ("m.instagram.com", "/p/DdUGmgAifcq"),
        ("www.instagram.com", "/p/DdUGmgAifcq"),
    ];

    assert_url_hosts_cover(URL_HOSTS, &cases, |url| parse_instagram_url(url).is_some());
}
