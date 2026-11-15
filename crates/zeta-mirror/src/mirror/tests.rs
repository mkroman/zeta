//! Unit tests for the shared mirror service.

use std::path::PathBuf;

use url::Url;

use super::*;
use crate::{MediaId, s3::S3, ytdlp::YtDlp};

/// Returns a mirror that never talks to S3 and has no manager running.
fn mirror_for_test() -> Mirror {
    Mirror::new(S3::for_test(), YtDlp::with_command("yt-dlp"), None, 2)
}

#[test]
fn test_in_flight_tracking() {
    let mirror = mirror_for_test();

    // Newly marked media returns the key that clears the mark.
    assert!(mirror.mark_in_flight("reddit", "123").is_some());
    // Media that is already being mirrored is not marked again.
    assert!(mirror.mark_in_flight("reddit", "123").is_none());
    assert!(mirror.mark_in_flight("reddit", "456").is_some());
    // The same id under a different prefix is a different piece of media.
    assert!(mirror.mark_in_flight("tiktok", "456").is_some());

    Mirror::clear_in_flight(&mirror.in_flight, ("reddit".to_string(), "123".to_string()));
    assert!(mirror.mark_in_flight("reddit", "123").is_some());
}

#[tokio::test]
async fn test_start_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let download_dir = dir.path().join("downloads");
    let mirror = Mirror::new(
        S3::for_test(),
        YtDlp::with_command("false"),
        Some(download_dir.clone()),
        2,
    );

    assert!(mirror.manager.get().is_none());
    assert!(mirror.start(), "the manager should start");
    assert!(mirror.manager.get().is_some());

    // A second start is a no-op: rebuilding the manager would recreate the removed
    // directory.
    std::fs::remove_dir(&download_dir).unwrap();
    assert!(mirror.start());
    assert!(
        !download_dir.exists(),
        "the second start rebuilt the download manager"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_start_disables_mirroring_when_the_directory_cannot_be_created() {
    let mirror = Mirror::new(
        S3::for_test(),
        YtDlp::with_command("false"),
        Some(PathBuf::from("/dev/null/downloads")),
        2,
    );

    assert!(!mirror.start(), "the manager should not start");
    assert!(matches!(mirror.manager.get(), Some(None)));
    // The failure is cached, so a later call does not retry.
    assert!(!mirror.start());
}

/// Returns a mirror whose S3 endpoint answers every request with a 404, so that the
/// fast-path check finds no mirror, together with the server that must outlive it.
async fn mirror_with_missing_objects() -> (Mirror, zeta_test_support::wiremock::MockServer) {
    let server = zeta_test_support::not_found_server().await;
    let endpoint = format!("http://{}", server.address());
    let mirror = Mirror::new(
        S3::with_endpoint(&endpoint),
        // `false` fails every download immediately, so the test never reaches the network.
        YtDlp::with_command("false"),
        None,
        2,
    );

    (mirror, server)
}

#[tokio::test]
async fn test_ensure_mirrored_starts_the_download_manager() {
    let (mirror, _server) = mirror_with_missing_objects().await;
    let base = Url::parse("https://pub.rwx.im/reddit").unwrap();
    let id = MediaId::parse("123").expect("the id is valid");

    // The manager is not started by a `loaded` hook anymore: the first request must start
    // it, or a URL delivered before `loaded` runs would be dropped.
    assert!(mirror.manager.get().is_none());

    let result = mirror
        .ensure_mirrored("reddit", &base, "https://v.redd.it/123", &id, |_| {})
        .await
        .expect("the fast-path check should succeed");

    assert!(result.is_none());
    assert!(
        mirror.download_manager().is_some(),
        "the first mirror request should start the download manager"
    );
}
