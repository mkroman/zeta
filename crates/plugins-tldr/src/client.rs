//! Client for the Kagi Summarize endpoint the browser extension calls.
//!
//! The endpoint is `kagi.com/mother/summary_labs`, the same-origin fetch the extension issues
//! when summarizing the current page. Authentication is the `kagi_session` session token, sent
//! three ways like the extension does: as the raw `Authorization` header, as the
//! `X-Kagi-Authorization` header the extension injects with a request rule, and as the
//! `kagi_session` cookie the browser attaches to same-origin requests. Requests are otherwise
//! shaped like the extension's: JSON content type, permissive accept, the browser fetch
//! metadata (`Sec-Fetch-*`) of a same-origin CORS call, and the `u=4` priority the browser
//! gives a background fetch.

use secrecy::{ExposeSecret, SecretString};
use url::Url;
use zeta_http::reqwest;
use zeta_plugin::accounts::PreferenceValue;

use crate::error::Error;
use crate::model::SummaryResponse;
use crate::preferences::SummaryType;

/// The summarize endpoint the Kagi browser extension calls.
const SUMMARY_LABS_ENDPOINT: &str = "https://kagi.com/mother/summary_labs";

/// The `Accept-Language` header sent when no language is configured.
pub(crate) const DEFAULT_LANGUAGE: &str = "en-US,en;q=0.9";

/// The `Accept` header of the extension's fetch.
const ACCEPT_ANY: &str = "*/*";
/// The `Priority` the browser gives the extension's background fetch.
const PRIORITY: &str = "u=4";

/// Client for the Kagi Summarize endpoint.
#[derive(Clone)]
pub(crate) struct SummaryClient {
    /// The shared HTTP client the requests go through.
    http: reqwest::Client,
    /// The `Accept-Language` header sent with requests.
    language: reqwest::header::HeaderValue,
    /// The `Authorization`/`X-Kagi-Authorization` header value of the token.
    authorization: reqwest::header::HeaderValue,
    /// The `Cookie` header value carrying the `kagi_session` token.
    cookie: reqwest::header::HeaderValue,
}

// Manual implementation so the session token is never printed.
impl std::fmt::Debug for SummaryClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SummaryClient")
            .field("language", &self.language)
            .finish_non_exhaustive()
    }
}

impl SummaryClient {
    /// Constructs a client over the shared HTTP client with the given session token.
    ///
    /// The token is the value of the `kagi_session` cookie from an authenticated browser
    /// session. The header values are built once here, so an unusable token or language fails
    /// plugin initialization instead of the first request.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidHeader`] when the token or language cannot be a header value.
    pub(crate) fn new(
        http: reqwest::Client,
        token: impl Into<SecretString>,
        language: &str,
    ) -> Result<Self, Error> {
        let token = token.into();
        let authorization = reqwest::header::HeaderValue::from_str(token.expose_secret())?;
        let cookie = reqwest::header::HeaderValue::from_str(&format!(
            "kagi_session={}",
            token.expose_secret()
        ))?;

        Ok(Self {
            http,
            language: reqwest::header::HeaderValue::from_str(language)?,
            authorization,
            cookie,
        })
    }

    /// Summarizes `page`, sending the extension's request shape.
    ///
    /// `summary_type` is always sent (the extension's selector always has a value);
    /// `target_language` is sent only when set, exactly like the extension omits its default.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the request fails, the response carries an error status, or the
    /// body cannot be parsed.
    pub(crate) async fn summarize(
        &self,
        page: &Url,
        summary_type: SummaryType,
        target_language: Option<&str>,
    ) -> Result<SummaryResponse, Error> {
        let request = self.request(SUMMARY_LABS_ENDPOINT, page, summary_type, target_language);
        let response = zeta_http::send(request).await?;
        let summary = zeta_http::parse_response(response).await?;

        Ok(summary)
    }

    /// Builds the summarize request against `endpoint`, the extension's fetch.
    ///
    /// The endpoint is a parameter so the recorded-session tests can point the client at a
    /// local server; production calls go through [`SummaryClient::summarize`]'s constant.
    fn request(
        &self,
        endpoint: &str,
        page: &Url,
        summary_type: SummaryType,
        target_language: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let mut params = vec![
            ("url", page.as_str()),
            ("summary_type", summary_type.as_str()),
        ];
        if let Some(language) = target_language {
            params.push(("target_language", language));
        }

        self.http
            .get(endpoint)
            .query(&params)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, ACCEPT_ANY)
            .header(reqwest::header::ACCEPT_LANGUAGE, self.language.clone())
            .header(reqwest::header::AUTHORIZATION, self.authorization.clone())
            .header("X-Kagi-Authorization", self.authorization.clone())
            .header(reqwest::header::COOKIE, self.cookie.clone())
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-origin")
            .header("Priority", PRIORITY)
    }

    /// Summarizes `page` against a test `base` URL, e.g. a mock server's.
    #[cfg(test)]
    pub(crate) async fn summarize_at(
        &self,
        base: &str,
        page: &Url,
        summary_type: SummaryType,
        target_language: Option<&str>,
    ) -> Result<SummaryResponse, Error> {
        let request = self.request(
            &format!("{base}/mother/summary_labs"),
            page,
            summary_type,
            target_language,
        );
        let response = zeta_http::send(request).await?;

        zeta_http::parse_response(response)
            .await
            .map_err(Error::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use zeta_test_support::wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    /// The endpoint path, relative to the mock server.
    const TEST_PATH: &str = "/mother/summary_labs";

    /// A client with a throwaway token.
    fn client() -> SummaryClient {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("the test client builds");

        SummaryClient::new(http, "test-token", DEFAULT_LANGUAGE).expect("the headers build")
    }

    #[tokio::test]
    async fn the_request_looks_like_the_extensions() {
        let server = MockServer::start().await;
        let body: serde_json::Value =
            serde_json::from_str(crate::tests::summary_fixture()).expect("the fixture parses");
        Mock::given(method("GET"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let client = client();
        let page = Url::parse("https://www.dr.dk/nyheder/udland/eu").expect("the page parses");
        let response = client
            .request(
                &format!("{}{TEST_PATH}", server.uri()),
                &page,
                SummaryType::Takeaway,
                Some("DE"),
            )
            .send()
            .await
            .expect("the request sends");

        assert_eq!(response.status(), 200);

        let requests = server
            .received_requests()
            .await
            .expect("the requests arrive");
        assert_eq!(requests.len(), 1);
        let request = &requests[0];

        assert_eq!(
            request.url.query(),
            Some(
                "url=https%3A%2F%2Fwww.dr.dk%2Fnyheder%2Fudland%2Feu&summary_type=takeaway&target_language=DE"
            ),
            "the parameters are percent-encoded like the extension's URLSearchParams"
        );

        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
        };

        assert_eq!(
            header("authorization"),
            "test-token",
            "the raw token is sent"
        );
        assert_eq!(header("x-kagi-authorization"), "test-token");
        assert_eq!(header("cookie"), "kagi_session=test-token");
        assert_eq!(header("content-type"), "application/json");
        assert_eq!(header("accept"), "*/*");
        assert_eq!(header("accept-language"), DEFAULT_LANGUAGE);
        assert_eq!(header("sec-fetch-dest"), "empty");
        assert_eq!(header("sec-fetch-mode"), "cors");
        assert_eq!(header("sec-fetch-site"), "same-origin");
        assert_eq!(header("priority"), "u=4");
    }

    #[tokio::test]
    async fn the_default_language_is_omitted_from_the_request() {
        let server = MockServer::start().await;
        let body: serde_json::Value =
            serde_json::from_str(crate::tests::summary_fixture()).expect("the fixture parses");
        Mock::given(method("GET"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let client = client();
        let page = Url::parse("https://example.com/article").expect("the page parses");
        client
            .summarize_at(&server.uri(), &page, SummaryType::Summary, None)
            .await
            .expect("the summary parses");

        let requests = server
            .received_requests()
            .await
            .expect("the requests arrive");
        assert_eq!(requests.len(), 1);
        assert!(
            !requests[0]
                .url
                .query_pairs()
                .any(|(key, _)| key == "target_language"),
            "the endpoint default is reached: no target_language was sent"
        );
    }

    #[tokio::test]
    async fn an_error_status_maps_to_the_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(TEST_PATH))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = client();
        let page = Url::parse("https://example.com/article").expect("the page parses");
        let error = client
            .summarize_at(&server.uri(), &page, SummaryType::Summary, None)
            .await
            .expect_err("the 401 is an error");

        assert!(
            matches!(error, Error::Api(_)),
            "the status maps to the API error: {error:?}"
        );
        assert_eq!(error.reply(), "invalid kagi session token");
    }
}
