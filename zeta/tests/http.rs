//! The public contract of the shared HTTP wrappers, exercised as an external consumer would:
//! status classification, `404` mapping and JSON decoding against a wiremock server.

#![cfg(feature = "http")]

use serde_json::json;
use zeta::config::HttpConfig;
use zeta::http::{self, ApiError};
use zeta_test_support::wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{any, method, path, query_param},
};

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Api(#[from] ApiError),
}

#[tokio::test]
async fn get_json_decodes_a_successful_response() {
    let server = MockServer::start().await;

    // The request the caller built is honored: path and query reach the server verbatim.
    Mock::given(method("GET"))
        .and(path("/weather"))
        .and(query_param("key", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "temp": 12.5 })))
        .mount(&server)
        .await;

    let client = http::build_client(&HttpConfig::default());
    let request = client.get(format!("{}/weather?key=secret", server.uri()));

    let decoded: serde_json::Value = http::get_json(request).await.unwrap();

    assert_eq!(decoded, json!({ "temp": 12.5 }));
}

#[tokio::test]
async fn get_json_reports_error_statuses_with_their_body() {
    let server = MockServer::start().await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let client = http::build_client(&HttpConfig::default());
    let request = client.get(server.uri());

    let error = http::get_json::<serde_json::Value>(request)
        .await
        .expect_err("the error status should fail the request");

    match error {
        ApiError::Status { status, body } => {
            assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(body, "boom");
        }
        other => panic!("expected a status error, got: {other:?}"),
    }
}

#[tokio::test]
async fn get_json_reports_invalid_json() {
    let server = MockServer::start().await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&server)
        .await;

    let client = http::build_client(&HttpConfig::default());
    let request = client.get(server.uri());

    let error = http::get_json::<serde_json::Value>(request)
        .await
        .expect_err("the invalid body should fail the request");

    assert!(matches!(error, ApiError::Deserialize(_)), "{error:?}");
}

#[tokio::test]
async fn get_json_or_404_maps_not_found() {
    let server = MockServer::start().await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let client = http::build_client(&HttpConfig::default());
    let request = client.get(server.uri());

    let error = http::get_json_or_404::<serde_json::Value, TestError>(request, TestError::NotFound)
        .await
        .expect_err("the missing resource should map to the not-found error");

    assert!(matches!(error, TestError::NotFound), "{error:?}");
}
