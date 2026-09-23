//! Shows the current weather for a location.
//!
//! The `.w <location>` command geocodes the location through the OpenWeatherMap geocoding API
//! (first match) and then queries the current weather, replying with the temperature, condition
//! description, humidity, wind speed, and pressure — unit labels follow the `units` setting
//! (metric by default; imperial switches °C to °F and m/s to mph). Condition descriptions are
//! requested in the `language` setting.
//!
//! The API key is set in `[plugins.openweathermap]`, falling back to the
//! `OPENWEATHERMAP_APP_ID` environment variable; a missing key fails plugin initialization and
//! the plugin is skipped at startup. Invoking `.w` without arguments uses the
//! `default_location` setting, or replies with usage when unset.

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::{error::RequestError, http, plugin::prelude::*};

/// Base URL for the OpenWeatherMap API.
const API_BASE_URL: &str = "https://api.openweathermap.org";

/// The unit system used for temperatures and wind speeds.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    /// Temperatures in Celsius and wind speeds in meters per second.
    #[default]
    Metric,
    /// Temperatures in Fahrenheit and wind speeds in miles per hour.
    Imperial,
    /// Temperatures in Kelvin and wind speeds in meters per second.
    Standard,
}

impl Units {
    /// Returns the value of the `units` API parameter.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Metric => "metric",
            Self::Imperial => "imperial",
            Self::Standard => "standard",
        }
    }

    /// Returns the temperature unit label.
    const fn temperature(self) -> &'static str {
        match self {
            Self::Metric => "°C",
            Self::Imperial => "°F",
            Self::Standard => "K",
        }
    }

    /// Returns the wind speed unit label.
    const fn wind_speed(self) -> &'static str {
        match self {
            Self::Imperial => "mph",
            Self::Metric | Self::Standard => "m/s",
        }
    }
}

/// Settings for the openweathermap plugin, from its `[plugins.openweathermap]` configuration
/// section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Settings {
    /// The OpenWeatherMap API key (the "app id").
    ///
    /// Falls back to the `OPENWEATHERMAP_APP_ID` environment variable when unset.
    #[serde(default)]
    pub app_id: Option<String>,
    /// The unit system used for temperatures and wind speeds.
    #[serde(default)]
    pub units: Units,
    /// The language used for weather condition descriptions.
    ///
    /// Defaults to the API's own default (English) when unset.
    #[serde(default)]
    pub language: Option<String>,
    /// The location used when the `.w` command is invoked without arguments.
    #[serde(default)]
    pub default_location: Option<String>,
}

/// The `.w` command.
const WEATHER: CommandSpec = CommandSpec::new(".w", "Show current weather for a location");

/// Plugin for querying weather data.
pub struct OpenWeatherMap {
    /// HTTP client for making API requests.
    client: reqwest::Client,
    /// OpenWeatherMap API key.
    app_id: String,
    /// The unit system used for temperatures and wind speeds.
    units: Units,
    /// The language used for weather condition descriptions.
    language: Option<String>,
    /// The location used when the command is invoked without arguments.
    default_location: Option<String>,
}

/// Errors that can occur during weather lookups.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurred while performing the HTTP request.
    #[error("request error: {0}")]
    Request(#[from] RequestError),
    /// The location could not be found via the geocoding API.
    #[error("location not found")]
    LocationNotFound,
    /// The API returned an error status or message.
    #[error("api error: {0}")]
    Api(String),
}

/// Result from the Geocoding API.
#[derive(Deserialize, Debug)]
struct GeocodingResult {
    /// Latitude.
    lat: f64,
    /// Longitude.
    lon: f64,
}

/// Result from the Current Weather Data API.
#[derive(Deserialize, Debug)]
struct WeatherResponse {
    /// City name.
    name: String,
    /// System parameters.
    sys: Sys,
    /// Main weather data (temperature, etc.).
    main: Main,
    /// Weather condition descriptions.
    weather: Vec<WeatherDescription>,
    /// Wind data.
    wind: Wind,
    /// Cloud coverage.
    clouds: Option<Clouds>,
}

/// System parameters (country, timestamps, etc.).
#[derive(Deserialize, Debug)]
struct Sys {
    /// Country code (e.g. "DK", "US").
    country: Option<String>,
}

/// Main weather parameters.
#[derive(Deserialize, Debug)]
struct Main {
    /// Temperature in the configured unit.
    temp: f64,
    /// Feels-like temperature in the configured unit.
    feels_like: f64,
    /// Humidity percentage.
    humidity: u8,
    /// Atmospheric pressure in hPa.
    pressure: u16,
}

/// Weather condition description.
#[derive(Deserialize, Debug)]
struct WeatherDescription {
    /// Weather condition within the group.
    description: String,
}

/// Wind statistics.
#[derive(Deserialize, Debug)]
struct Wind {
    /// Wind speed in the configured unit.
    speed: f64,
    /// Wind gust in the configured unit.
    gust: Option<f64>,
}

/// Cloud coverage.
#[derive(Deserialize, Debug)]
struct Clouds {
    /// Cloudiness, %.
    all: u8,
}

#[async_trait]
impl Plugin<Context> for OpenWeatherMap {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.command(WEATHER);
        let app_id = resolve_secret(settings.app_id.as_deref(), "OPENWEATHERMAP_APP_ID")?;
        let client = http::build_client(&ctx.config.http);

        Ok(Self {
            client,
            app_id,
            units: settings.units,
            language: settings.language.clone(),
            default_location: settings.default_location.clone(),
        })
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let args = command.args();

        let location = if args.trim().is_empty() {
            let Some(location) = self.default_location.as_deref() else {
                client.send_privmsg(channel, notice("Usage: .w\x0f <location>"))?;

                return Ok(());
            };

            location
        } else {
            args.trim()
        };

        match self.fetch_weather(location).await {
            Ok(weather) => {
                client.send_privmsg(channel, format_weather(&weather, self.units))?;
            }
            Err(Error::LocationNotFound) => {
                client.send_privmsg(channel, notice("Location not found"))?;
            }
            Err(e) => {
                warn!(error = ?e, "openweathermap error");
                client.send_privmsg(channel, notice(e))?;
            }
        }

        Ok(())
    }
}

impl OpenWeatherMap {
    /// Fetches weather for a given location string.
    ///
    /// This involves two steps:
    /// 1. Geocoding the location string to coordinates (lat, lon).
    /// 2. Fetching the weather data for those coordinates.
    async fn fetch_weather(&self, location: &str) -> Result<WeatherResponse, Error> {
        let geo = self.geocode(location).await?;
        self.current_weather(geo.lat, geo.lon).await
    }

    /// Geocodes a location query to coordinates.
    async fn geocode(&self, query: &str) -> Result<GeocodingResult, Error> {
        debug!(%query, "geocoding");
        let url = format!("{API_BASE_URL}/geo/1.0/direct");
        let params = [("q", query), ("limit", "1"), ("appid", &self.app_id)];

        let response = http::send(self.client.get(&url).query(&params)).await?;

        let results: Vec<GeocodingResult> = http::parse_response(response)
            .await
            .map_err(|error| contextual_api_error("geocoding failed", error))?;
        results.into_iter().next().ok_or(Error::LocationNotFound)
    }

    /// Fetches current weather data for specific coordinates.
    async fn current_weather(&self, lat: f64, lon: f64) -> Result<WeatherResponse, Error> {
        debug!(lat, lon, "fetching current weather");
        let url = format!("{API_BASE_URL}/data/2.5/weather");
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

        let response = http::send(self.client.get(&url).query(&params)).await?;

        http::parse_response(response)
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
    }
}

/// Formats the weather response into a natural language string.
fn format_weather(w: &WeatherResponse, units: Units) -> String {
    let temp_unit = units.temperature();
    let speed_unit = units.wind_speed();

    let location = w
        .sys
        .country
        .as_ref()
        .map_or_else(|| w.name.clone(), |c| format!("{}, {}", w.name, c));

    let conditions = if w.weather.is_empty() {
        "unknown conditions".to_string()
    } else {
        w.weather
            .iter()
            .map(|d| d.description.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };

    let wind_info = match w.wind.gust {
        Some(g) if g > 0.0 => format!(
            "{:.1} {speed_unit} (gusts: {:.1} {speed_unit})",
            w.wind.speed, g
        ),
        _ => format!("{:.1} {speed_unit}", w.wind.speed),
    };

    let mut extra_info = Vec::new();
    extra_info.push(format!("Wind: \x0f{wind_info}\x0310"));
    extra_info.push(format!("Humidity: \x0f{}%\x0310", w.main.humidity));
    extra_info.push(format!("Pressure: \x0f{} hPa\x0310", w.main.pressure));

    if let Some(clouds) = &w.clouds {
        extra_info.push(format!("Cloud coverage: \x0f{}%\x0310", clouds.all));
    }

    notice(format!(
        "Right now in \x0f{}\x0310 it's \x0f{:.1} {temp_unit}\x0310 (feels like \x0f{:.1} {temp_unit}\x0310) with \x0f{}\x0310. {}",
        location,
        w.main.temp,
        w.main.feels_like,
        conditions,
        extra_info.join(". ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a weather response for formatting tests.
    fn test_response() -> WeatherResponse {
        WeatherResponse {
            name: "Copenhagen".to_string(),
            sys: Sys {
                country: Some("DK".to_string()),
            },
            main: Main {
                temp: 12.5,
                feels_like: 11.0,
                humidity: 72,
                pressure: 1013,
            },
            weather: vec![WeatherDescription {
                description: "light rain".to_string(),
            }],
            wind: Wind {
                speed: 3.0,
                gust: Some(6.5),
            },
            clouds: Some(Clouds { all: 75 }),
        }
    }

    #[tokio::test]
    async fn request_errors_redact_the_url_but_keep_the_cause() {
        let address = http::refused_address();
        let error = reqwest::Client::new()
            .get(format!("http://{address}/?appid=secret"))
            .send()
            .await
            .expect_err("nothing listens on that address");

        let error = Error::from(RequestError::from(error));

        assert!(!error.to_string().contains("secret"), "{error}");
        assert!(!format!("{error:?}").contains("secret"), "{error:?}");
    }

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.app_id.is_none());
            assert_eq!(settings.units, Units::Metric);
            assert!(settings.language.is_none());
            assert!(settings.default_location.is_none());
        }
        deserialize: {
            "app_id": "secret",
            "units": "imperial",
            "language": "da",
            "default_location": "Copenhagen",
        } assert: {
            assert_eq!(settings.app_id.as_deref(), Some("secret"));
            assert_eq!(settings.units, Units::Imperial);
            assert_eq!(settings.language.as_deref(), Some("da"));
            assert_eq!(settings.default_location.as_deref(), Some("Copenhagen"));
        }
    }

    #[test]
    fn formats_metric_weather() {
        let formatted = format_weather(&test_response(), Units::Metric);

        assert!(formatted.contains("Copenhagen, DK"), "{formatted}");
        assert!(formatted.contains("12.5 °C"), "{formatted}");
        assert!(formatted.contains("feels like \x0f11.0 °C"), "{formatted}");
        assert!(
            formatted.contains("3.0 m/s (gusts: 6.5 m/s)"),
            "{formatted}"
        );
    }

    #[test]
    fn formats_imperial_weather() {
        let formatted = format_weather(&test_response(), Units::Imperial);

        assert!(formatted.contains("12.5 °F"), "{formatted}");
        assert!(formatted.contains("3.0 mph"), "{formatted}");
    }
}
