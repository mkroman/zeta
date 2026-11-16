//! Tests for the Spotify plugin: settings, link and URI parsing, and artist formatting.

use similar_asserts::assert_eq;
use url::Url;

use zeta_plugin::irc;
use zeta_test_support::settings_tests;

use crate::Settings;
use crate::commands::join_artists;
use crate::model::ArtistSimple;
use crate::urls::{parse_spotify_uris, parse_spotify_url};

settings_tests! {
    Settings,
    settings,
    default: {
        assert!(settings.client_id.is_none());
        assert!(settings.client_secret.is_none());
    }
    deserialize: {
        "client_id": "id",
        "client_secret": "secret",
    } assert: {
        assert_eq!(settings.client_id.as_deref(), Some("id"));
        assert_eq!(settings.client_secret.as_deref(), Some("secret"));
    }
}

#[test]
fn parses_bare_spotify_uris() {
    assert_eq!(
        parse_spotify_uris(
            "listen to spotify:track:4uLU6hMCjMI75M1A2tKUQC and \
             spotify:album:1DFixLWuPkv3KT3TnV35m3"
        ),
        [
            ("track", "4uLU6hMCjMI75M1A2tKUQC"),
            ("album", "1DFixLWuPkv3KT3TnV35m3"),
        ]
    );

    // Each component must be present and use the URI alphabet.
    for text in [
        "spotify::id",
        "spotify:track:",
        "spotify:tr4ck:id",
        // Plain links are handled by `parse_spotify_url`, not the URI parser.
        "https://open.spotify.com/track/id",
    ] {
        assert_eq!(
            parse_spotify_uris(text),
            Vec::<(&str, &str)>::new(),
            "{text}"
        );
    }

    // The id ends at the first character outside the alphabet.
    assert_eq!(
        parse_spotify_uris("spotify:track:abc! rest"),
        [("track", "abc")]
    );

    // Matches are non-overlapping.
    assert_eq!(
        parse_spotify_uris("spotify:spotify:track:id"),
        [("spotify", "track")]
    );
}

#[test]
fn parses_spotify_resource_urls() {
    fn parse(url: &str) -> Option<(String, String)> {
        parse_spotify_url(&Url::parse(url).unwrap())
            .map(|(kind, id)| (kind.to_string(), id.to_string()))
    }

    assert_eq!(
        parse("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC"),
        Some(("track".to_string(), "4uLU6hMCjMI75M1A2tKUQC".to_string()))
    );
    assert_eq!(
        parse("https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3?si=abc"),
        Some(("album".to_string(), "1DFixLWuPkv3KT3TnV35m3".to_string()))
    );

    // Root paths and deeper nesting are not resources.
    assert_eq!(parse("https://open.spotify.com/"), None);
    assert_eq!(
        parse("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC/extra"),
        None
    );
}

#[test]
fn joins_artists_into_a_sentence() {
    let artists = |names: &[&str]| {
        names
            .iter()
            .map(|name| ArtistSimple {
                name: (*name).to_string(),
            })
            .collect::<Vec<_>>()
    };

    let cases = [
        ("empty", &[][..], ""),
        ("single", &["Aurora"][..], "Aurora"),
        ("pair", &["Aurora", "Sigrid"][..], "Aurora and Sigrid"),
    ];

    for (case, names, expected) in cases {
        assert_eq!(
            irc::strip_formatting(&join_artists(&artists(names))),
            expected,
            "{case}"
        );
    }

    // The comma sits between two color codes, so `strip_formatting` would swallow it; pin the exact
    // layout for the three-name variant to keep the Oxford separator covered.
    assert_eq!(
        join_artists(&artists(&["Aurora", "Sigrid", "Agnes Obel"])),
        "\x0fAurora\x0310, \x0fSigrid\x0310, and \x0fAgnes Obel\x0310"
    );
}
