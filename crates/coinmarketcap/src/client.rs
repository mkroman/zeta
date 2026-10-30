//! Client for the CoinMarketCap API.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::debug;

use crate::error::{Error, ParseError, RequestError};
use crate::model::{Coin, CoinQuery, Envelope, Fiat, Listing, QuoteData};

/// Base URL of the CoinMarketCap Pro API.
pub const API_BASE_URL: &str = "https://pro-api.coinmarketcap.com";

/// The HTTP header that carries the CoinMarketCap API key on requests.
const API_KEY_HEADER: HeaderName = HeaderName::from_static("x-cmc_pro_api_key");

/// The number of top cryptocurrencies (by market cap) cached for symbol and name lookups.
const COIN_MAP_LIMIT: u32 = 1000;

/// The maximum number of fiat currencies to fetch from the fiat map endpoint.
///
/// The endpoint allows up to 5000 results per request and charges one credit regardless of
/// size, so all supported currencies are fetched in a single request.
const FIAT_MAP_LIMIT: u32 = 5000;

/// The maximum number of characters of an unparseable response body that reaches the logs.
const LOGGED_BODY_LENGTH: usize = 512;

/// Configuration for the CoinMarketCap client.
#[derive(Clone, Debug)]
pub struct Config {
    /// The duration before a request times out.
    pub timeout: Duration,
    /// The `User-Agent` header sent with requests.
    pub user_agent: String,
}

impl Default for Config {
    /// The default configuration: a 30-second timeout, and the crate's name and version as the
    /// user agent.
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            user_agent: concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")).to_owned(),
        }
    }
}

/// A client for the CoinMarketCap API.
#[derive(Debug)]
pub struct Client {
    /// HTTP client for CoinMarketCap API requests, with the API key set as a default header.
    http: reqwest::Client,
    /// The base URL the client issues requests against; `API_BASE_URL` outside of tests.
    base: String,
}

impl Client {
    /// Builds a client for the CoinMarketCap API with the default configuration. Use
    /// [`Client::with_config`] to customize the timeout and user agent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidApiKey`] if the API key is not a valid HTTP header value, and
    /// [`Error::Build`] if the client could not be built.
    pub fn new(api_key: impl Into<String>) -> Result<Self, Error> {
        Self::with_config(api_key, &Config::default())
    }

    /// Builds a client for the CoinMarketCap API, with the API key and the expected response
    /// type set as default headers, and the timeout and user agent taken from `config`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidApiKey`] if the API key is not a valid HTTP header value, and
    /// [`Error::Build`] if the client could not be built.
    pub fn with_config(api_key: impl Into<String>, config: &Config) -> Result<Self, Error> {
        Self::with_base(api_key, config, API_BASE_URL)
    }

    /// Builds a client against `base` instead of the public API endpoint — for tests, and for
    /// proxying the API through a self-hosted endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidApiKey`] if the API key is not a valid HTTP header value, and
    /// [`Error::Build`] if the client could not be built.
    pub fn with_base(
        api_key: impl Into<String>,
        config: &Config,
        base: &str,
    ) -> Result<Self, Error> {
        let headers = HeaderMap::from_iter([
            (ACCEPT, HeaderValue::from_static("application/json")),
            (
                API_KEY_HEADER,
                HeaderValue::from_str(&api_key.into()).map_err(Error::InvalidApiKey)?,
            ),
        ]);

        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(config.timeout)
            .user_agent(&config.user_agent)
            .default_headers(headers)
            .build()
            .map_err(RequestError::from)
            .map_err(Error::Build)?;

        Ok(Self {
            http,
            base: base.to_owned(),
        })
    }

    /// Fetches the map of the top cryptocurrencies (by market cap rank) from the CoinMarketCap
    /// API.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the request fails, the API responds with an error status, or
    /// the response cannot be parsed.
    pub async fn coin_map(&self) -> Result<Vec<Coin>, Error> {
        debug!(limit = COIN_MAP_LIMIT, "fetching the top cryptocurrencies");

        let parsed: Envelope<Vec<Coin>> = self
            .get_json(
                "/v1/cryptocurrency/map",
                &[
                    ("limit", COIN_MAP_LIMIT.to_string()),
                    ("sort", "cmc_rank".to_string()),
                ],
            )
            .await?;

        Ok(parsed.data.unwrap_or_default())
    }

    /// Fetches the ranked list of the top cryptocurrencies with their latest market quotes from
    /// the CoinMarketCap API, so a single credited call yields the whole price table.
    ///
    /// Returns the coins in market cap rank order, quoted in USD. The endpoint is charged one
    /// call credit per 250 coins returned (rounded up), so a limit of 100 costs a single
    /// credit per refresh.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the request fails, the API responds with an error status, or
    /// the response cannot be parsed.
    pub async fn listings(&self, limit: u32) -> Result<Vec<Listing>, Error> {
        debug!(limit, "fetching the top cryptocurrency listings");

        let parsed: Envelope<Vec<Listing>> = self
            .get_json(
                "/v3/cryptocurrency/listings/latest",
                &[
                    ("start", "1".to_string()),
                    ("limit", limit.to_string()),
                    ("convert", "USD".to_string()),
                ],
            )
            .await?;

        Ok(parsed.data.unwrap_or_default())
    }

    /// Fetches the fiat currencies supported for price conversion from the fiat map endpoint.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the request fails, the API responds with an error status, or
    /// the response cannot be parsed.
    pub async fn fiat_map(&self) -> Result<Vec<Fiat>, Error> {
        debug!("fetching the supported fiat currencies");

        let parsed: Envelope<Vec<Fiat>> = self
            .get_json("/v1/fiat/map", &[("limit", FIAT_MAP_LIMIT.to_string())])
            .await?;

        Ok(parsed.data.unwrap_or_default())
    }

    /// Fetches the quote for a single coin in the given conversion currency.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] when the API does not know the coin, and an [`Error`] for
    /// request, status or parse failures.
    pub async fn get_quote(&self, coin: CoinQuery, convert: &str) -> Result<QuoteData, Error> {
        let mut query: Vec<(&str, String)> = vec![("convert", convert.to_string())];

        match coin {
            CoinQuery::Id(id) => query.push(("id", id.to_string())),
            CoinQuery::Symbol(symbol) => query.push(("symbol", symbol)),
        }

        debug!(coinmarketcap.query = ?query, "requesting coin quote");

        let parsed: Envelope<HashMap<String, QuoteData>> = self
            .get_json("/v1/cryptocurrency/quotes/latest", &query)
            .await?;

        // The data payload is keyed by the requested identifier (symbol or id); a single coin is
        // requested, so the first (and only) value is the quote.
        parsed
            .data
            .and_then(|data| data.into_values().next())
            .ok_or(Error::NotFound)
    }

    /// Performs a GET request against `path` with `query`, deserializing a successful response
    /// into `T`.
    ///
    /// Non-success statuses are mapped to an error by [`api_error`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the request fails, the response status is not a success, or
    /// the response cannot be parsed.
    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, Error> {
        let request = self.http.get(format!("{}{path}", self.base)).query(query);

        let response = request.send().await.map_err(request_error)?;
        let status = response.status();
        let text = response.text().await.map_err(request_error)?;

        if !status.is_success() {
            return Err(api_error(status, &text));
        }

        decode(&text).map_err(Error::Deserialize)
    }
}

/// Wraps `error` in a [`RequestError`], whose URL keeps no credential from the query string.
///
/// The failed request is logged once with its redacted URL and error type: the caller reports
/// the short rendering, so this is the only line that names the request.
fn request_error(error: reqwest::Error) -> Error {
    let error = RequestError::from(error);

    if let Some(url) = error.url() {
        debug!(
            url.full = %url,
            error.type = error.error_type(),
            exception.message = %error.full(),
            "request failed"
        );
    }

    Error::Request(error)
}

/// Deserializes a value of type `T` from a JSON string.
///
/// The offending body is logged if parsing fails.
///
/// # Errors
///
/// Returns the parse error, including the path to the offending part of the document, if
/// `text` is not valid JSON for `T`.
fn decode<T: DeserializeOwned>(text: &str) -> Result<T, ParseError> {
    let deserializer = &mut serde_json::Deserializer::from_str(text);

    serde_path_to_error::deserialize(deserializer).inspect_err(|error| {
        let body = text.chars().take(LOGGED_BODY_LENGTH).collect::<String>();

        debug!(
            exception.message = ?error,
            http.response.body = %body,
            "could not deserialize json response"
        );
    })
}

/// Builds an API error from a non-success response, preserving the API error message if present.
///
/// Bad requests (e.g. an unknown symbol or id) are reported as a miss rather than an error.
fn api_error(status: StatusCode, body: &str) -> Error {
    let message = serde_json::from_str::<Envelope<Value>>(body)
        .ok()
        .filter(|envelope| envelope.status.error_code != "0")
        .and_then(|envelope| envelope.status.error_message);

    if status == StatusCode::BAD_REQUEST {
        debug!(?message, "the api rejected the request");

        return Error::NotFound;
    }

    message.map_or_else(
        || Error::Api(format!("unexpected response status {status}")),
        Error::Api,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_quote_response() {
        let text = r#"{"data":{"BTC":{"id":1,"name":"Bitcoin","symbol":"BTC","slug":"bitcoin","quote":{"USD":{"price":97231.504,"volume_24h":48000000000,"percent_change_1h":0.5,"percent_change_24h":-1.2,"percent_change_7d":10.25,"last_updated":"2026-09-09T00:00:00.000Z"}}}},"status":{"timestamp":"2026-09-09T00:00:00.000Z","error_code":0,"error_message":null,"elapsed":12,"credit_count":1}}"#;

        let parsed: Envelope<HashMap<String, QuoteData>> = decode(text).unwrap();
        let quote = parsed.data.unwrap().remove("BTC").unwrap();

        assert_eq!(quote.name, "Bitcoin");
        assert_eq!(quote.symbol, "BTC");
        assert_eq!(quote.quote["USD"].price, Some(97231.504));
        assert_eq!(quote.quote["USD"].percent_change_24h, Some(-1.2));
    }

    #[test]
    fn decodes_listings_response() {
        let text = r#"{"data":[{"id":1027,"name":"Ethereum","symbol":"ETH","slug":"ethereum","cmc_rank":2,"num_market_pairs":9001,"circulating_supply":120000000,"total_supply":120000000,"max_supply":null,"infinite_supply":false,"date_added":"2015-08-07T00:00:00.000Z","last_updated":"2026-09-29T11:13:00.000Z","quote":[{"id":2781,"symbol":"USD","price":2650.32,"volume_24h":1000000000,"percent_change_1h":0.4,"percent_change_24h":1.2,"percent_change_7d":3.4,"last_updated":"2026-09-29T11:13:00.000Z"}]}],"status":{"timestamp":"2026-09-29T11:13:55.525Z","error_code":"0","error_message":null,"elapsed":12,"credit_count":1}}"#;

        let parsed: Envelope<Vec<Listing>> = decode(text).unwrap();
        let listings = parsed.data.unwrap();

        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].symbol, "ETH");
        assert_eq!(listings[0].name, "Ethereum");
        assert_eq!(listings[0].cmc_rank, Some(2));
        assert_eq!(listings[0].quote[0].symbol, "USD");
        assert_eq!(listings[0].quote[0].price, Some(2650.32));
    }

    #[test]
    fn decodes_coin_map_response() {
        let text = r#"{"data":[{"id":1,"rank":1,"name":"Bitcoin","symbol":"BTC","slug":"bitcoin","is_active":1,"first_historical_data":"2013-04-28T18:47:21.000Z","last_historical_data":"2020-05-05T20:44:01.000Z","platform":null},{"id":1027,"rank":2,"name":"Ethereum","symbol":"ETH","slug":"ethereum","is_active":1,"first_historical_data":"2015-08-07T14:49:30.000Z","last_historical_data":"2020-05-05T20:44:02.000Z","platform":null}],"status":{"timestamp":"2026-09-13T00:00:00.000Z","error_code":0,"error_message":null,"elapsed":12,"credit_count":1}}"#;

        let parsed: Envelope<Vec<Coin>> = decode(text).unwrap();
        let coins = parsed.data.unwrap();

        assert_eq!(coins.len(), 2);
        assert_eq!(coins[0].symbol, "BTC");
        assert_eq!(coins[1].id, 1027);
    }

    #[test]
    fn decodes_fiat_map_response() {
        let text = r#"{"data":[{"id":2781,"name":"United States Dollar","sign":"$","symbol":"USD"},{"id":2787,"name":"Chinese Yuan","sign":"¥","symbol":"CNY"}],"status":{"timestamp":"2026-09-13T00:00:00.000Z","error_code":0,"error_message":null,"elapsed":12,"credit_count":1}}"#;

        let parsed: Envelope<Vec<Fiat>> = decode(text).unwrap();
        let fiats = parsed.data.unwrap();

        assert_eq!(fiats.len(), 2);
        assert_eq!(fiats[0].symbol, "USD");
        assert_eq!(fiats[0].sign, "$");
        assert_eq!(fiats[1].symbol, "CNY");
        assert_eq!(fiats[1].sign, "¥");
    }

    #[test]
    fn maps_bad_request_to_not_found() {
        let text = r#"{"status":{"timestamp":"2026-09-09T00:00:00.000Z","error_code":400,"error_message":"Invalid value for \"symbol\"","elapsed":5,"credit_count":1}}"#;

        let err = api_error(StatusCode::BAD_REQUEST, text);

        assert!(matches!(err, Error::NotFound));
    }

    #[test]
    fn maps_bad_request_with_unparseable_body_to_not_found() {
        let err = api_error(StatusCode::BAD_REQUEST, "<html>gateway error</html>");

        assert!(matches!(err, Error::NotFound));
    }

    #[test]
    fn builds_api_error_from_error_payload() {
        let text = r#"{"status":{"timestamp":"2026-09-09T00:00:00.000Z","error_code":"1001","error_message":"This API Key is invalid.","elapsed":5,"credit_count":1}}"#;

        let err = api_error(StatusCode::UNAUTHORIZED, text);

        assert_eq!(
            err.to_string(),
            "Could not retrieve coin information: This API Key is invalid."
        );
    }

    #[test]
    fn maps_deserialize_failures_to_a_user_facing_message() {
        let err = decode::<Envelope<Value>>("{")
            .map_err(Error::Deserialize)
            .expect_err("malformed json");

        assert_eq!(
            err.to_string(),
            "Could not parse the response from the CoinMarketCap API"
        );
    }

    #[test]
    fn builds_api_error_from_unparseable_payload() {
        let err = api_error(StatusCode::INTERNAL_SERVER_ERROR, "<html>oops</html>");

        assert!(err.to_string().contains("500"), "{err:?}");
    }

    #[tokio::test]
    async fn request_errors_redact_the_url_but_keep_the_cause() {
        let (error, address) = zeta_test_support::refused_get("key=secret").await;

        let error = RequestError::from(error);

        zeta_test_support::assert_redactions(
            &error.to_string(),
            error.full(),
            &format!("{error:?}"),
            &address,
        );

        // The chain ends at the wrapper, so nothing that walks `source()` can reach a raw error.
        assert!(std::error::Error::source(&error).is_none(), "{error:?}");
    }
}
