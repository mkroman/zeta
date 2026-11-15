//! Unit tests for the S3 client.
//!
//! The tests here cover the client's configuration, its object URLs and its signed headers; the
//! request and retry paths are exercised through the download manager's tests.

use std::time::Duration;

use aws_sigv4::http_request::SignableBody;
use sha2::{Digest, Sha256};

use super::*;
use crate::config::MirrorConfig;

#[tokio::test(start_paused = true)]
async fn test_presence_check_is_bounded() {
    use zeta_test_support::wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};

    // An endpoint that accepts the request but never answers it: only the presence check's
    // own timeout can end the wait. The server must not come from wiremock's pool: the
    // test ends with the request still in flight, and a pooled server would be borrowed by
    // another test, which then records this request as one of its own.
    let server = MockServer::builder().start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_mins(60)))
        .mount(&server)
        .await;

    let s3 = S3::with_endpoint(&format!("http://{}", server.address()));
    let error = s3
        .object_exists("tiktok/123.mp4")
        .await
        .expect_err("the presence check must time out");

    assert!(
        matches!(&error, Error::Io(error) if error.kind() == std::io::ErrorKind::TimedOut),
        "{error:?}"
    );
}

#[test]
fn blank_region_falls_back_to_the_environment_then_auto() {
    let config = MirrorConfig {
        s3_access_key_id: Some("access".to_string()),
        s3_secret_access_key: Some("secret".into()),
        s3_bucket_name: Some("bucket".to_string()),
        s3_endpoint: Some("http://127.0.0.1:9".to_string()),
        s3_region: Some("   ".to_string()),
        ..MirrorConfig::default()
    };

    // A blank region counts as unset and resolves exactly like an omitted one. The test
    // does not mutate the process environment; it computes the environment's contribution
    // with the same helper the client uses.
    let expected = zeta_utils::settings::resolve_optional_setting(Some(" "), "S3_REGION")
        .unwrap_or_else(|| "auto".to_string());

    assert_eq!(
        S3::try_new(&config)
            .expect("the configuration is complete")
            .region,
        expected
    );

    let config = MirrorConfig {
        s3_region: Some("eu-west-1".to_string()),
        ..config
    };

    // A configured region wins over the environment and the default.
    assert_eq!(
        S3::try_new(&config)
            .expect("the configuration is complete")
            .region,
        "eu-west-1"
    );
}

#[test]
fn test_object_url_path_style() {
    let s3 = S3::with_endpoint("http://localhost:9000");

    assert_eq!(
        s3.object_url("reddit/123.mp4").unwrap().as_str(),
        "http://localhost:9000/test/reddit/123.mp4"
    );
}

#[test]
fn test_signing_headers_include_content_hash() {
    let s3 = S3::with_endpoint("http://localhost:9000");
    let url = s3.object_url("tiktok/123.mp4").unwrap();

    let headers = s3
        .signing_headers("HEAD", &url, SignableBody::empty())
        .expect("could not sign request");

    assert!(
        headers
            .iter()
            .any(|(name, _)| *name == "x-amz-content-sha256"),
        "signature must include the payload hash: {headers:?}"
    );
    assert!(
        headers.iter().any(|(name, _)| *name == "authorization"),
        "signature must include the authorization header: {headers:?}"
    );
}

#[test]
fn test_precomputed_body_hash_matches_bytes() {
    let s3 = S3::with_endpoint("http://localhost:9000");
    let url = s3.object_url("tiktok/123.mp4").unwrap();
    let digest = const_hex::encode(Sha256::digest(b"hello"));

    let hash_of = |body: SignableBody<'_>| {
        s3.signing_headers("PUT", &url, body)
            .expect("could not sign request")
            .into_iter()
            .find(|(name, _)| *name == "x-amz-content-sha256")
            .map(|(_, value)| value)
            .expect("signature must include the payload hash")
    };

    assert_eq!(hash_of(SignableBody::Precomputed(digest.clone())), digest);
    assert_eq!(
        hash_of(SignableBody::Precomputed(digest)),
        hash_of(SignableBody::Bytes(b"hello"))
    );
}
