//! Unit tests for the settings, summary formatting, proxy URLs, and the media pipeline.

use super::*;

use similar_asserts::assert_eq;
use url::Url;
use zeta_plugin::irc;
use zeta_test_support::settings_tests;

use crate::format::format_summary;
use crate::media::proxy_media_url;
use crate::urls::{InstagramLink, MediaKind, media_url, parse_instagram_url};

settings_tests! {
    Settings,
    settings,
    default: {
        assert_eq!(settings.caption_length, 150);
        assert!(settings.mirror.prefix.is_none());
        assert!(settings.mirror.public_url_base.is_none());
        assert!(settings.session_cookie.is_none());
        assert!(settings.metadata_proxy.is_none());
    }
    deserialize: {
        "caption_length": 100,
        "prefix": "~meta/instagram",
        "public_url_base": "https://pub.example.com/instagram",
        "session_cookie": "1234567890%3Aexample",
        "metadata_proxy": "https://d.example.com",
    } assert: {
        assert_eq!(settings.caption_length, 100);
        assert_eq!(settings.mirror.prefix.as_deref(), Some("~meta/instagram"));
        assert_eq!(
            settings.mirror.public_url_base.as_deref(),
            Some("https://pub.example.com/instagram")
        );
        assert_eq!(
            settings.session_cookie.as_deref(),
            Some("1234567890%3Aexample")
        );
        assert_eq!(settings.metadata_proxy.as_deref(), Some("https://d.example.com"));
    }
}

/// The pre-rename `title_length` key keeps configuring the caption limit.
#[test]
fn legacy_title_length_key_is_accepted() {
    let settings: Settings = serde_json::from_value(serde_json::json!({ "title_length": 100 }))
        .expect("the legacy key should deserialize");

    assert_eq!(settings.caption_length, 100);
}

/// Returns the details of a media with both a caption and an author, to be partially
/// overridden per case.
fn full_details() -> MediaDetails {
    MediaDetails {
        author: Some("user.name".to_string()),
        caption: Some("Some caption & more".to_string()),
    }
}

#[test]
fn test_format_summary() {
    let cases = [
        (
            "a caption and an author",
            full_details(),
            "reel",
            Some("“Some caption & more” is an Instagram reel by user.name".to_string()),
        ),
        (
            "a long caption",
            MediaDetails {
                caption: Some("a".repeat(200)),
                ..full_details()
            },
            "post",
            Some(format!(
                "“{}…” is an Instagram post by user.name",
                "a".repeat(150)
            )),
        ),
        (
            "a caption without an author",
            MediaDetails {
                author: None,
                caption: Some("Just a title".to_string()),
            },
            "post",
            Some("“Just a title” is an Instagram post".to_string()),
        ),
        (
            "an author without a caption",
            MediaDetails {
                caption: None,
                ..full_details()
            },
            "story",
            Some("Instagram story by user.name".to_string()),
        ),
        (
            "neither an author nor a caption",
            MediaDetails {
                author: None,
                caption: None,
            },
            "post",
            None,
        ),
        (
            // Captions carry line breaks, which an IRC message cannot contain: everything
            // after the first one would be lost.
            "a caption with collapsible whitespace",
            MediaDetails {
                caption: Some("\x0310First line\n\nsecond\tline\r\nthird\x0f".to_string()),
                ..full_details()
            },
            "post",
            Some("“First line second line third” is an Instagram post by user.name".to_string()),
        ),
    ];

    for (case, details, kind, expected) in cases {
        assert_eq!(
            format_summary(&details, kind, 150)
                .as_deref()
                .map(irc::strip_formatting),
            expected,
            "for {case}"
        );
    }
}

#[test]
fn test_proxy_media_url() {
    assert_eq!(
        proxy_media_url("https://d.example.com", MediaKind::Post, "DdUGmgAifcq"),
        "https://d.example.com/p/DdUGmgAifcq"
    );
    assert_eq!(
        proxy_media_url("https://d.example.com/", MediaKind::Reel, "DbfDT_9xiU3"),
        "https://d.example.com/reel/DbfDT_9xiU3"
    );
}

/// Drives the plugin's pipeline against real Instagram links.
///
/// Needs network access, so it is `#[ignore]`d. Instagram serves a login wall to requests it
/// scores as bot traffic — e.g. from datacenter IPs — and some media is not anonymously
/// accessible at all, so a missing summary is not a failure: the summary is asserted on when
/// the metadata sources carried details; otherwise the test logs the outcome and passes.
#[tokio::test]
#[ignore = "needs network access"]
async fn live_media_links() {
    let client = http::emulated::builder(None)
        .expect("the headers are valid")
        .redirect(Policy::limited(4))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("the http client builds");

    let instagram = Instagram {
        client,
        mirror: None,
        settings: Settings::default(),
        session_cookie: None,
        metadata_proxy: None,
        last_request: Mutex::new(None),
        session: TtlCache::new(SESSION_TTL),
        details: TtlMap::new_with_negative_ttl(
            DETAILS_TTL,
            MISSING_DETAILS_TTL,
            DETAILS_CACHE_CAPACITY,
        ),
    };

    let urls = [
        "https://www.instagram.com/p/DdUGmgAifcq",
        "https://www.instagram.com/p/DdUfj1USxzi",
        "https://www.instagram.com/reel/DbfDT_9xiU3/",
        "https://www.instagram.com/reel/DaklCKgDnIe/",
    ];

    for url in urls {
        let url = Url::parse(url).unwrap();

        let InstagramLink::Media { kind, id } = parse_instagram_url(&url).unwrap() else {
            panic!("expected a media link for {url}");
        };

        let canonical = media_url(kind, &id);
        println!("== {url} → {canonical}");

        let details = instagram
            .details
            .get_or_refresh(id.clone(), || async {
                instagram.fetch_media_details(&id, &canonical, kind).await
            })
            .await
            .unwrap_or(None);

        match details
            .as_ref()
            .and_then(|details| format_summary(details, kind.label(), 150))
        {
            Some(summary) => println!("   summary: {summary}"),
            None => println!("   no summary — no source carried details"),
        }
    }
}
