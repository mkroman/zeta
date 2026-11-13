//! Request helpers for the OpenWeatherMap geocoding and current weather endpoints.

use tracing::debug;

use super::model::{GeocodingResult, WeatherResponse};
use super::{Error, OpenWeatherMap};
use zeta_http as http;

/// The direct-geocoding endpoint location names are resolved against.
const GEOCODING_URL: &str = "https://api.openweathermap.org/geo/1.0/direct";

/// The current-weather endpoint coordinates are fetched from.
const WEATHER_URL: &str = "https://api.openweathermap.org/data/2.5/weather";

impl OpenWeatherMap {
    /// Fetches weather for a given location string.
    ///
    /// This involves two steps:
    /// 1. Geocoding the location string to coordinates (lat, lon).
    /// 2. Fetching the weather data for those coordinates.
    pub(super) async fn fetch_weather(&self, location: &str) -> Result<WeatherResponse, Error> {
        let geo = self.geocode(location).await?;
        self.current_weather(geo.lat, geo.lon).await
    }

    /// Geocodes a location query to coordinates.
    async fn geocode(&self, query: &str) -> Result<GeocodingResult, Error> {
        debug!(search.query = %query, "geocoding");
        let params = [("q", query), ("limit", "1"), ("appid", &self.app_id)];

        let results: Vec<GeocodingResult> =
            http::get_json(self.client.get(GEOCODING_URL).query(&params))
                .await
                .map_err(|error| contextual_api_error("geocoding failed", error))?;
        results.into_iter().next().ok_or(Error::LocationNotFound)
    }

    /// Fetches current weather data for specific coordinates.
    async fn current_weather(&self, lat: f64, lon: f64) -> Result<WeatherResponse, Error> {
        debug!(
            geo.location.lat = lat,
            geo.location.lon = lon,
            "fetching current weather"
        );
        let lat_s = lat.to_string();
        let lon_s = lon.to_string();
        let mut params = vec![
            ("lat", lat_s.as_str()),
            ("lon", lon_s.as_str()),
            ("appid", self.app_id.as_str()),
            ("units", self.units.as_str()),
        ];

        if let Some(language) = self.language.as_deref() {
            params.push(("lang", language));
        }

        http::get_json(self.client.get(WEATHER_URL).query(&params))
            .await
            .map_err(|error| contextual_api_error("weather fetch failed", error))
    }
}

/// Maps an API error into an [`Error`], prefixing status errors with `context`.
fn contextual_api_error(context: &str, error: http::ApiError) -> Error {
    match error {
        http::ApiError::Status { status, .. } => Error::Api(format!("{context}: {status}")),
        http::ApiError::Request(error) => Error::Request(error),
        http::ApiError::Deserialize(error) => Error::Api(error.to_string()),
        other => Error::Api(other.to_string()),
    }
}
