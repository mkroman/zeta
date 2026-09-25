//! Client for the unwall.app API.

use std::time::Duration;

use serde::Deserialize;
use url::Url;

use super::error::Error;
use crate::http;

/// The default base URL of the unwall API.
pub const DEFAULT_API_BASE: &str = "https://api.unwall.app/";

/// The tested domains and counters, as reported by `GET /bootstrap`.
#[derive(Debug, Deserialize)]
struct Bootstrap {
    #[serde(rename = "testedDomains", default)]
    tested_domains: Vec<String>,
}

/// Parses `input` as a base URL, ensuring it ends in a slash so its endpoints can be
/// [`Url::join`]ed onto it.
///
/// # Errors
///
/// Returns [`Error::BaseUrl`] if `input` cannot be parsed.
pub fn base_url(input: &str) -> Result<Url, Error> {
    let mut base = Url::parse(input)?;

    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }

    Ok(base)
}

/// Client for the unwall.app API.
///
/// The API needs no authentication and is rate limited to 120 requests per minute, far above
/// what an IRC channel produces.
#[derive(Debug, Clone)]
pub struct UnwallClient {
    /// The HTTP client.
    http: reqwest::Client,
    /// How long an article submission may take, overriding the client timeout: a first mirror
    /// of the article runs server-side and can take tens of seconds.
    submit_timeout: Duration,
    /// The API base URL, ending in a slash.
    api_base: Url,
}

impl UnwallClient {
    /// Creates a client for the unwall API at `api_base`.
    #[must_use]
    pub const fn new(http: reqwest::Client, submit_timeout: Duration, api_base: Url) -> Self {
        Self {
            http,
            submit_timeout,
            api_base,
        }
    }

    /// Returns the URL of the API endpoint `name`.
    fn endpoint(&self, name: &str) -> Result<Url, Error> {
        self.api_base.join(name).map_err(|_| Error::InvalidUrl)
    }

    /// Submits `article` to unwall.app, triggering the server-side mirror of the article.
    ///
    /// The request carries the submit timeout instead of the client's: a first mirror of the
    /// article runs server-side and can take tens of seconds, while the shared HTTP timeout is
    /// tuned for the regular API calls. The response body — the mirrored HTML, often hundreds
    /// of kilobytes — is deliberately not read: a success status is all the bot needs, and the
    /// reader link is deterministic (`https://unwall.app/<host>/<path>`), so nothing else has
    /// to be parsed out of it.
    ///
    /// Only ever called for articles not in the cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Request`] if the request could not be sent, and [`Error::Status`]
    /// when the API answers with an error status.
    pub async fn submit(&self, article: &Url) -> Result<(), Error> {
        let request = self
            .http
            .get(self.endpoint("fetch")?)
            .timeout(self.submit_timeout)
            .query(&[("url", article.as_str())]);

        let response = http::send(request).await?;
        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            Err(Error::Status(status.as_u16()))
        }
    }

    /// Returns the sites unwall.app has tested, trying `GET /bootstrap` and falling back to
    /// `GET /tested-domains` — the same pair the unwall frontend tries.
    ///
    /// # Errors
    ///
    /// Returns the error of the failed fallback request when both endpoints fail.
    pub async fn tested_domains(&self) -> Result<Vec<String>, Error> {
        match http::get_json::<Bootstrap>(self.http.get(self.endpoint("bootstrap")?)).await {
            Ok(bootstrap) if !bootstrap.tested_domains.is_empty() => Ok(bootstrap.tested_domains),
            _ => Ok(http::get_json::<Vec<String>>(
                self.http.get(self.endpoint("tested-domains")?),
            )
            .await?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HttpConfig;

    /// The submit timeout handed to the client under test.
    const SUBMIT_TIMEOUT: Duration = Duration::from_secs(60);
    use zeta_test_support::wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    #[tokio::test]
    async fn submit_accepts_a_successful_response_without_reading_the_body() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/fetch"))
            .and(query_param(
                "url",
                "https://www.bloomberg.com/news/articles/2026-09-25/trump",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"html\":\"...\"}"))
            .mount(&server)
            .await;

        let client = UnwallClient::new(
            http::build_client(&HttpConfig::default()),
            SUBMIT_TIMEOUT,
            base_url(&server.uri()).unwrap(),
        );

        client
            .submit(
                &"https://www.bloomberg.com/news/articles/2026-09-25/trump"
                    .parse()
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn submit_reports_error_statuses() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(400).set_body_string("{\"error\":\"Invalid URL\"}"))
            .mount(&server)
            .await;

        let client = UnwallClient::new(
            http::build_client(&HttpConfig::default()),
            SUBMIT_TIMEOUT,
            base_url(&server.uri()).unwrap(),
        );

        let error = client
            .submit(&"https://www.bloomberg.com/news/a".parse().unwrap())
            .await
            .unwrap_err();

        assert!(matches!(error, Error::Status(400)));
    }

    #[tokio::test]
    async fn tested_domains_decode_the_bootstrap_response() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/bootstrap"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "wallsCleared": 2_613_209,
                    "wallsPerSecond": 0.214,
                    "testedDomains": ["ft.com", "www.bloomberg.com"],
                })),
            )
            .mount(&server)
            .await;

        let client = UnwallClient::new(
            http::build_client(&HttpConfig::default()),
            SUBMIT_TIMEOUT,
            base_url(&server.uri()).unwrap(),
        );

        assert_eq!(
            client.tested_domains().await.unwrap(),
            ["ft.com", "www.bloomberg.com"]
        );
    }

    #[tokio::test]
    async fn tested_domains_fall_back_to_the_tested_domains_endpoint() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/bootstrap"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tested-domains"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(["ft.com"])))
            .mount(&server)
            .await;

        let client = UnwallClient::new(
            http::build_client(&HttpConfig::default()),
            SUBMIT_TIMEOUT,
            base_url(&server.uri()).unwrap(),
        );

        assert_eq!(client.tested_domains().await.unwrap(), ["ft.com"]);
    }

    #[test]
    fn a_base_url_without_a_trailing_slash_still_resolves_endpoints() {
        let client = UnwallClient::new(
            http::build_client(&HttpConfig::default()),
            SUBMIT_TIMEOUT,
            base_url("https://api.unwall.app").unwrap(),
        );

        assert_eq!(
            client.endpoint("fetch").unwrap().as_str(),
            "https://api.unwall.app/fetch"
        );
    }

    /// Live smoke test against unwall.app, to catch contract drift that the mocked tests
    /// cannot. Run with `cargo test -p zeta --all-features -- --ignored unwall::`.
    #[tokio::test]
    #[ignore = "needs network access"]
    async fn live_api_contract_smoke() {
        let client = UnwallClient::new(
            http::build_client(&HttpConfig::default()),
            SUBMIT_TIMEOUT,
            base_url(DEFAULT_API_BASE).unwrap(),
        );

        client
            .submit(
                &"https://www.bloomberg.com/news/articles/2026-09-25/\
                  trump-asked-zelenskyy-to-meet-putin-in-moscow-for-peace-talks"
                    .parse()
                    .unwrap(),
            )
            .await
            .unwrap();

        let domains = client.tested_domains().await.unwrap();

        assert!(domains.contains(&"ft.com".to_string()));
    }
}
