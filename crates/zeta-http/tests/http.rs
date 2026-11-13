//! The public contract of the shared HTTP wrappers, exercised as an external consumer would:
//! status classification, `404` mapping, and JSON and text decoding against a wiremock server.

#![cfg(feature = "http")]

use serde_json::json;
use zeta_http as http;
use zeta_http::HttpConfig;
use zeta_http::{ApiError, RedirectError};
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
async fn get_text_returns_a_successful_body() {
    let server = MockServer::start().await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>hello</html>"))
        .mount(&server)
        .await;

    let client = http::build_client(&HttpConfig::default());
    let body = http::get_text(client.get(server.uri())).await.unwrap();

    assert_eq!(body, "<html>hello</html>");
}

#[tokio::test]
async fn get_text_reports_error_statuses_with_their_body() {
    let server = MockServer::start().await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(404).set_body_string("not here"))
        .mount(&server)
        .await;

    let client = http::build_client(&HttpConfig::default());
    let error = http::get_text(client.get(server.uri()))
        .await
        .expect_err("an error page must not be returned as content");

    match error {
        ApiError::Status { status, body } => {
            assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
            assert_eq!(body, "not here");
        }
        other => panic!("expected a status error, got: {other:?}"),
    }
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

#[tokio::test]
async fn redirect_location_returns_the_location_header() {
    let server = MockServer::start().await;

    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", "https://example.com/target?key=secret"),
        )
        .mount(&server)
        .await;

    // The shared builder disables redirects, so the first response still carries the header.
    let client = http::build_client(&HttpConfig::default());
    let url = http::redirect_location(client.get(server.uri()))
        .await
        .unwrap();

    assert_eq!(url.as_str(), "https://example.com/target?key=secret");
}

#[tokio::test]
async fn redirect_location_reports_a_missing_header() {
    let server = MockServer::start().await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(302))
        .mount(&server)
        .await;

    let client = http::build_client(&HttpConfig::default());
    let error = http::redirect_location(client.get(server.uri()))
        .await
        .expect_err("a redirect without a location must fail");

    assert!(matches!(error, RedirectError::MissingLocation), "{error:?}");
}

#[tokio::test]
async fn redirect_location_reports_an_unparseable_header() {
    let server = MockServer::start().await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(302).insert_header("location", "not a url"))
        .mount(&server)
        .await;

    let client = http::build_client(&HttpConfig::default());
    let error = http::redirect_location(client.get(server.uri()))
        .await
        .expect_err("a relative location must fail to parse as an absolute url");

    assert!(matches!(error, RedirectError::LocationUrl(_)), "{error:?}");
}
