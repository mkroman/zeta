//! OpenWeatherMap integration plugin.
//!
//! This plugin allows users to query current weather information via the OpenWeatherMap API
//! using the `.w` command.

use serde::Deserialize;
use tracing::{debug, warn};

use crate::{http, plugin::prelude::*};

/// Base URL for the OpenWeatherMap API.
const API_BASE_URL: &str = "https://api.openweathermap.org";
/// Constant for converting Kelvin to Celsius.
const KELVIN: f64 = 273.15;

/// Plugin for querying weather data.
pub struct OpenWeatherMap {
    /// HTTP client for making API requests.
    client: reqwest::Client,
    /// Command handler for the `.w` command.
    command: ZetaCommand,
    /// OpenWeatherMap API key.
    app_id: String,
}

/// Errors that can occur during weather lookups.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurred while performing the HTTP request.
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
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
    /// Temperature in Kelvin.
    temp: f64,
    /// Feels-like temperature in Kelvin.
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
    /// Wind speed in m/s.
    speed: f64,
    /// Wind gust in m/s.
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
    fn new(_ctx: &Context) -> Self {
        let app_id = std::env::var("OPENWEATHERMAP_APP_ID")
            .expect("missing OPENWEATHERMAP_APP_ID environment variable");
        let client = http::build_client();
        let command = ZetaCommand::new(".w");

        Self {
            client,
            command,
            app_id,
        }
    }

    fn name() -> Name {
        Name::from("openweathermap")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("1.1")
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(args) = self.command.parse(user_message)
        {
            let location = args.trim();
            if location.is_empty() {
                client.send_privmsg(channel, "\x0310> Usage: .w\x0f <location>")?;
                return Ok(());
            }

            match self.fetch_weather(location).await {
                Ok(weather) => {
                    client.send_privmsg(channel, format_weather(&weather))?;
                }
                Err(Error::LocationNotFound) => {
                    client.send_privmsg(channel, "\x0310> Location not found")?;
                }
                Err(e) => {
                    warn!(error = ?e, "openweathermap error");
                    client.send_privmsg(channel, format!("\x0310> Error: {e}"))?;
                }
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

        let response = self.client.get(&url).query(&params).send().await?;

        if !response.status().is_success() {
            return Err(Error::Api(format!(
                "geocoding failed: {}",
                response.status()
            )));
        }

        let results: Vec<GeocodingResult> = response.json().await?;
        results.into_iter().next().ok_or(Error::LocationNotFound)
    }

    /// Fetches current weather data for specific coordinates.
    async fn current_weather(&self, lat: f64, lon: f64) -> Result<WeatherResponse, Error> {
        debug!(lat, lon, "fetching current weather");
        let url = format!("{API_BASE_URL}/data/2.5/weather");
        let lat_s = lat.to_string();
        let lon_s = lon.to_string();
        let params = [
            ("lat", lat_s.as_str()),
            ("lon", lon_s.as_str()),
            ("appid", self.app_id.as_str()),
        ];

        let response = self.client.get(&url).query(&params).send().await?;

        if !response.status().is_success() {
            return Err(Error::Api(format!(
                "weather fetch failed: {}",
                response.status()
            )));
        }

        response.json().await.map_err(Error::from)
    }
}

/// Formats the weather response into a natural language string.
fn format_weather(w: &WeatherResponse) -> String {
    let temp = w.main.temp - KELVIN;
    let feels_like = w.main.feels_like - KELVIN;

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
        Some(g) if g > 0.0 => format!("{:.1} m/s (gusts: {:.1} m/s)", w.wind.speed, g),
        _ => format!("{:.1} m/s", w.wind.speed),
    };

    let mut extra_info = Vec::new();
    extra_info.push(format!("Wind: \x0f{wind_info}\x0310"));
    extra_info.push(format!("Humidity: \x0f{}%\x0310", w.main.humidity));
    extra_info.push(format!("Pressure: \x0f{} hPa\x0310", w.main.pressure));

    if let Some(clouds) = &w.clouds {
        extra_info.push(format!("Cloud coverage: \x0f{}%\x0310", clouds.all));
    }

    format!(
        "\x0310> Right now in \x0f{}\x0310 it's \x0f{:.1} °C\x0310 (feels like \x0f{:.1} °C\x0310) with \x0f{}\x0310. {}",
        location,
        temp,
        feels_like,
        conditions,
        extra_info.join(". ")
    )
}
