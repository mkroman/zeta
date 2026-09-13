//! Client for the CoinMarketCap API.

use std::collections::HashMap;

use reqwest::StatusCode;
use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::debug;

use super::error::Error;
use super::model::{Coin, CoinQuery, Envelope, Fiat, QuoteData};
use crate::http;
use crate::plugin::prelude::{ZetaError, plugin_err};

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

/// Client for the CoinMarketCap API.
pub struct Client {
    /// HTTP client for CoinMarketCap API requests, with the API key set as a default header.
    inner: reqwest::Client,
}

impl Client {
    /// Builds a client for the CoinMarketCap API, with the API key and the expected response
    /// type set as default headers.
    ///
    /// # Errors
    ///
    /// Returns [`ZetaError`] if the API key is not a valid HTTP header value, or the client
    /// could not be built.
    pub fn new(api_key: &str) -> Result<Self, ZetaError> {
        let headers = HeaderMap::from_iter([
            (ACCEPT, HeaderValue::from_static("application/json")),
            (
                API_KEY_HEADER,
                HeaderValue::from_str(api_key).map_err(plugin_err)?,
            ),
        ]);

        let inner = http::client::builder()
            .default_headers(headers)
            .build()
            .map_err(plugin_err)?;

        Ok(Self { inner })
    }

    /// Fetches the map of the top cryptocurrencies (by market cap rank) from the CoinMarketCap
    /// API.
    pub async fn coin_map(&self) -> Result<Vec<Coin>, Error> {
        debug!(limit = COIN_MAP_LIMIT, "fetching the top cryptocurrencies");

        let response = self
            .inner
            .get(format!("{API_BASE_URL}/v1/cryptocurrency/map"))
            .query(&[
                ("limit", COIN_MAP_LIMIT.to_string()),
                ("sort", "cmc_rank".to_string()),
            ])
            .send()
            .await
            .map_err(Error::Request)?;

        let status = response.status();
        let text = response.text().await.map_err(Error::Request)?;

        if !status.is_success() {
            return Err(api_error(status, &text));
        }

        let parsed: Envelope<Vec<Coin>> = decode_response(&text)?;

        Ok(parsed.data.unwrap_or_default())
    }

    /// Fetches the fiat currencies supported for price conversion from the CoinMarketCap API.
    pub async fn fiat_map(&self) -> Result<Vec<Fiat>, Error> {
        debug!("fetching the supported fiat currencies");

        let response = self
            .inner
            .get(format!("{API_BASE_URL}/v1/fiat/map"))
            .query(&[("limit", FIAT_MAP_LIMIT.to_string())])
            .send()
            .await
            .map_err(Error::Request)?;

        let status = response.status();
        let text = response.text().await.map_err(Error::Request)?;

        if !status.is_success() {
            return Err(api_error(status, &text));
        }

        let parsed: Envelope<Vec<Fiat>> = decode_response(&text)?;

        Ok(parsed.data.unwrap_or_default())
    }

    /// Fetches the quote for a single coin in the given conversion currency.
    pub async fn get_quote(&self, coin: CoinQuery, convert: &str) -> Result<QuoteData, Error> {
        let mut query: Vec<(&str, String)> = vec![("convert", convert.to_string())];

        match coin {
            CoinQuery::Id(id) => query.push(("id", id.to_string())),
            CoinQuery::Symbol(symbol) => query.push(("symbol", symbol)),
        }

        debug!(?query, "requesting coin quote");

        let response = self
            .inner
            .get(format!("{API_BASE_URL}/v1/cryptocurrency/quotes/latest"))
            .query(&query)
            .send()
            .await
            .map_err(Error::Request)?;

        let status = response.status();
        let text = response.text().await.map_err(Error::Request)?;

        if !status.is_success() {
            return Err(api_error(status, &text));
        }

        let parsed: Envelope<HashMap<String, QuoteData>> = decode_response(&text)?;

        // The data payload is keyed by the requested identifier (symbol or id); a single coin is
        // requested, so the first (and only) value is the quote.
        parsed
            .data
            .and_then(|data| data.into_values().next())
            .ok_or(Error::NotFound)
    }
}

/// Builds an API error from a non-success response, preserving the API error message if present.
///
/// Bad requests (e.g. an unknown symbol or id) are reported as a miss rather than an error.
fn api_error(status: StatusCode, body: &str) -> Error {
    let message = serde_json::from_str::<Envelope<Value>>(body)
        .ok()
        .filter(|envelope| envelope.status.error_code != 0)
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

/// Deserializes a response body into `T`, logging parse failures.
fn decode_response<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    let deserializer = &mut serde_json::Deserializer::from_str(text);

    serde_path_to_error::deserialize(deserializer)
        .inspect_err(|err| debug!(?err, %text, "could not deserialize response"))
        .map_err(Error::Deserialize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_quote_response() {
        let text = r#"{"data":{"BTC":{"id":1,"name":"Bitcoin","symbol":"BTC","slug":"bitcoin","quote":{"USD":{"price":97231.504,"volume_24h":48000000000,"percent_change_1h":0.5,"percent_change_24h":-1.2,"percent_change_7d":10.25,"last_updated":"2026-09-09T00:00:00.000Z"}}}},"status":{"timestamp":"2026-09-09T00:00:00.000Z","error_code":0,"error_message":null,"elapsed":12,"credit_count":1}}"#;

        let parsed: Envelope<HashMap<String, QuoteData>> = decode_response(text).unwrap();
        let quote = parsed.data.unwrap().remove("BTC").unwrap();

        assert_eq!(quote.name, "Bitcoin");
        assert_eq!(quote.symbol, "BTC");
        assert_eq!(quote.quote["USD"].price, Some(97231.504));
        assert_eq!(quote.quote["USD"].percent_change_24h, Some(-1.2));
    }

    #[test]
    fn decodes_coin_map_response() {
        let text = r#"{"data":[{"id":1,"rank":1,"name":"Bitcoin","symbol":"BTC","slug":"bitcoin","is_active":1,"first_historical_data":"2013-04-28T18:47:21.000Z","last_historical_data":"2020-05-05T20:44:01.000Z","platform":null},{"id":1027,"rank":2,"name":"Ethereum","symbol":"ETH","slug":"ethereum","is_active":1,"first_historical_data":"2015-08-07T14:49:30.000Z","last_historical_data":"2020-05-05T20:44:02.000Z","platform":null}],"status":{"timestamp":"2026-09-13T00:00:00.000Z","error_code":0,"error_message":null,"elapsed":12,"credit_count":1}}"#;

        let parsed: Envelope<Vec<Coin>> = decode_response(text).unwrap();
        let coins = parsed.data.unwrap();

        assert_eq!(coins.len(), 2);
        assert_eq!(coins[0].symbol, "BTC");
        assert_eq!(coins[1].id, 1027);
    }

    #[test]
    fn decodes_fiat_map_response() {
        let text = r#"{"data":[{"id":2781,"name":"United States Dollar","sign":"$","symbol":"USD"},{"id":2787,"name":"Chinese Yuan","sign":"¥","symbol":"CNY"}],"status":{"timestamp":"2026-09-13T00:00:00.000Z","error_code":0,"error_message":null,"elapsed":12,"credit_count":1}}"#;

        let parsed: Envelope<Vec<Fiat>> = decode_response(text).unwrap();
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
        let text = r#"{"status":{"timestamp":"2026-09-09T00:00:00.000Z","error_code":1001,"error_message":"This API Key is invalid.","elapsed":5,"credit_count":1}}"#;

        let err = api_error(StatusCode::UNAUTHORIZED, text);

        assert_eq!(err.to_string(), "This API Key is invalid.");
    }

    #[test]
    fn builds_api_error_from_unparseable_payload() {
        let err = api_error(StatusCode::INTERNAL_SERVER_ERROR, "<html>oops</html>");

        assert!(err.to_string().contains("500"), "{err:?}");
    }
}
