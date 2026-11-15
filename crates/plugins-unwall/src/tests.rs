//! Cross-cutting tests for the unwall plugin.

use std::time::Duration;

use clap::CommandFactory;
use zeta_plugin::irc::{COLOR, RESET};
use zeta_test_support::settings_tests;

use super::commands::join_and;
use super::media::resolve_reply;
use super::{Error, Settings};
use crate::args::Opts;

#[test]
fn cli_definition_is_valid() {
    // clap reports contradictory definitions through asserts; evaluating them here turns a
    // bad definition into a test failure instead of a panic in debug builds.
    Opts::command().debug_assert();
}

settings_tests! {
    Settings,
    settings,
    default: {
        assert_eq!(settings.api_base, "https://api.unwall.app/");
        assert_eq!(settings.reader_base, "https://unwall.app/");
        assert_eq!(settings.submit_timeout, Duration::from_mins(5));
    }
    deserialize: {
        "api_base": "https://unwall.example/api",
        "reader_base": "https://unwall.example/",
        "submit_timeout": "90s",
    } assert: {
        assert_eq!(settings.api_base, "https://unwall.example/api");
        assert_eq!(settings.reader_base, "https://unwall.example/");
        assert_eq!(settings.submit_timeout, Duration::from_secs(90));
    }
}

#[test]
fn joins_with_a_final_and() {
    assert_eq!(join_and(&[]), "");
    assert_eq!(join_and(&["a".into()]), format!("{RESET}a"));
    assert_eq!(
        join_and(&["a".into(), "b".into()]),
        format!("{RESET}a{COLOR} and{RESET} b")
    );
    assert_eq!(
        join_and(&["a".into(), "b".into(), "c".into()]),
        format!("{RESET}a{COLOR},{RESET} b{COLOR} and{RESET} c")
    );
}

/// Returns a request failure with a reply-grade `Display`.
fn request_error() -> Error {
    let error = zeta_http::reqwest::Client::new()
        .get("not a url")
        .build()
        .expect_err("a relative URL cannot be built into a request");

    Error::Request(zeta_http::RequestError::from(error))
}

#[test]
fn resolve_reply_uses_the_domain_text() {
    assert_eq!(
        resolve_reply(&Error::Status(503)),
        "unwall could not fetch the article (HTTP 503)"
    );
    assert_eq!(
        resolve_reply(&Error::InvalidUrl),
        "not a usable article URL"
    );
}

#[test]
fn resolve_reply_surfaces_a_reply_grade_request_error() {
    let error = request_error();

    assert_eq!(resolve_reply(&error), error.to_string());
}

#[test]
fn resolve_reply_hides_internal_errors() {
    let reply = resolve_reply(&Error::BaseUrl(url::ParseError::EmptyHost));

    assert_eq!(reply, "could not unwall the article");
    assert!(!reply.contains("base URL"));
}
