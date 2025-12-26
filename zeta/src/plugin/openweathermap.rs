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
    /// Main weather data (temperature, etc.).
    main: Main,
    /// Weather condition descriptions.
    weather: Vec<WeatherDescription>,
    /// Wind data.
    wind: Wind,
}

/// Main weather parameters.
#[derive(Deserialize, Debug)]
struct Main {
    /// Temperature in Kelvin.
    temp: f64,
    /// Feels-like temperature in Kelvin.
    feels_like: f64,
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

#[async_trait]
impl Plugin for OpenWeatherMap {
    fn new() -> Self {
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
        Version::from("1.0")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
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
        debug!(lat = &lat, lon = &lon, "fetching current weather");
        let url = format!("{API_BASE_URL}/data/2.5/weather");
        let params = [
            ("lat", lat.to_string()),
            ("lon", lon.to_string()),
            ("appid", self.app_id.clone()),
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

/// Formats the weather response into an IRC-friendly string.
fn format_weather(w: &WeatherResponse) -> String {
    let temp = w.main.temp - KELVIN;
    let feels_like = w.main.feels_like - KELVIN;
    let wind_speed = w.wind.speed;
    let wind_gust = w.wind.gust.unwrap_or(0.0);

    let mut result = format!(
        "Right now in\x0f {}\x0310 it's\x0f {:.1} °C\x0310 (feels like\x0f {:.1} °C\x0310)",
        w.name, temp, feels_like
    );

    if !w.weather.is_empty() {
        let weather_string = w
            .weather
            .iter()
            .map(|weather| format!("\x0f{}\x0310", weather.description))
            .collect::<Vec<_>>()
            .join(" and ");
        result.push_str(" with ");
        result.push_str(&weather_string);
    }

    result.push_str(&format!(
        ". Wind:\x0f {:.1} m/s\x0310, gusts:\x0f {:.1} m/s\x0310",
        wind_speed, wind_gust
    ));

    format!("\x0310> {}", result)
}
