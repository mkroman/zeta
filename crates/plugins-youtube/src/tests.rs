//! Tests for settings, URL recognition, and reply formatting.

use zeta_plugin::Chain;
use zeta_plugin::irc;
use zeta_test_support::{api_error, settings_tests};

use super::format::{format_video_message, search_error_reply, video_error_reply};
use super::urls::is_youtube_url;
use super::*;

settings_tests! {
    Settings,
    settings,
    default: {
        assert!(settings.api_key.is_none());
        assert_eq!(settings.region_code, "US");
        assert_eq!(settings.safe_search, SafeSearch::None);
        assert!(settings.enable_command);
    }
    deserialize: {
        "api_key": "secret",
        "region_code": "DK",
        "safe_search": "strict",
        "enable_command": false,
    } assert: {
        assert_eq!(settings.api_key.as_deref(), Some("secret"));
        assert_eq!(settings.region_code, "DK");
        assert_eq!(settings.safe_search, SafeSearch::Strict);
        assert!(!settings.enable_command);
    }
}

/// Returns a video with the given live status, duration, and concurrent viewer count.
fn test_video(
    live_broadcast_content: &str,
    duration: Option<&str>,
    concurrent_viewers: Option<&str>,
) -> Video {
    Video {
        snippet: Some(Snippet {
            title: "Test Video".to_string(),
            channel_title: "Test Channel".to_string(),
            category_id: "10".to_string(),
            live_broadcast_content: live_broadcast_content.to_string(),
        }),
        statistics: None,
        content_details: duration.map(|duration| ContentDetails {
            duration: duration.to_string(),
        }),
        live_streaming_details: concurrent_viewers.map(|viewers| LiveStreamingDetails {
            concurrent_viewers: Some(viewers.to_string()),
        }),
    }
}

#[test]
fn formats_search_failures() {
    assert_eq!(
        search_error_reply(&Error::NoResults),
        "could not search youtube: no results"
    );
    assert_eq!(search_error_reply(&api_error()), "could not search youtube");
}

#[test]
fn api_failures_keep_their_source_for_logs() {
    let error: Error = api_error();
    let message = Chain(&error).to_string();

    assert!(
        message.len() > error.to_string().len(),
        "the log line keeps the source chain: {message}"
    );
}

#[test]
fn formats_video_failures() {
    assert_eq!(
        video_error_reply(&Error::NoResults),
        "could not look up the video: no results"
    );
    assert_eq!(
        video_error_reply(&api_error()),
        "could not look up the video"
    );
}

#[test]
fn only_youtube_urls_are_treated_as_links_by_the_command() {
    assert!(is_youtube_url("https://youtu.be/dQw4w9WgXcQ"));
    assert!(is_youtube_url(
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
    ));
    assert!(is_youtube_url("https://youtube.com/shorts/l4s8y-O_ols"));

    assert!(!is_youtube_url("https://example.com/watch?v=dQw4w9WgXcQ"));
    assert!(!is_youtube_url("rust language"));
    assert!(!is_youtube_url(""));
}

#[test]
fn formats_video_message() {
    let video = test_video("none", Some("PT1H2M20S"), None);

    assert_eq!(
        format_video_message(&video, "Music", 123_456),
        "\x0310> “\x0fTest Video\x0310” is a\x0f 1h 2m 20s\x0310 video by\x0f Test Channel\x0310 with\x0f 123,456\x0310 views",
    );
}

#[test]
fn unescapes_html_entities_in_video_metadata() {
    let mut video = test_video("none", Some("PT1H2M20S"), None);
    let snippet = video
        .snippet
        .as_mut()
        .expect("the test video has a snippet");
    snippet.title = "Fish &amp; Chips".to_string();
    snippet.channel_title = "Ben &amp; Jerry".to_string();

    assert_eq!(
        irc::strip_formatting(&format_video_message(&video, "Film &amp; Animation", 1)),
        "> “Fish & Chips” is a 1h 2m 20s video by Ben & Jerry with 1 view",
    );
}

#[test]
fn formats_video_message_variants() {
    let cases = [
        (
            "unknown duration",
            test_video("none", None, None),
            "Music",
            1,
            "is an unknown duration video by Test Channel with 1 view",
        ),
        (
            "live stream prefers concurrent viewers",
            test_video("live", None, Some("1234")),
            "Music",
            42,
            "is a Music live stream by Test Channel with 1,234 viewers",
        ),
        (
            "vowel category takes an article",
            test_video("live", None, Some("1234")),
            "Education",
            42,
            "is an Education live stream by Test Channel with 1,234 viewers",
        ),
        (
            "upcoming stream falls back to total views",
            test_video("upcoming", None, None),
            "Music",
            42,
            "is a Music live stream by Test Channel with 42 views",
        ),
    ];

    for (case, video, category, view_count, expected) in cases {
        let formatted = irc::strip_formatting(&format_video_message(&video, category, view_count));

        assert!(
            formatted.starts_with("> “Test Video”"),
            "{case}: {formatted}"
        );
        assert!(formatted.contains(expected), "{case}: {formatted}");
    }
}
