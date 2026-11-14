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

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use tracing::warn;

use zeta_http::RequestError;
use zeta_http::reqwest;
use zeta_plugin::irc::{COLOR, PLAIN_REPLY_PREFIX, RESET, plain_reply, send_reply};
use zeta_plugin::{Client, CommandEvent, Context, ZetaError, plugin, resolve_secret};
use zeta_utils::irc;
use zeta_utils::text::IteratorJoinExt;

mod api;
mod model;

use self::model::WeatherResponse;

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
#[serde(default)]
pub struct Settings {
    /// The OpenWeatherMap API key (the "app id").
    ///
    /// Falls back to the `OPENWEATHERMAP_APP_ID` environment variable when unset.
    pub app_id: Option<String>,
    /// The unit system used for temperatures and wind speeds.
    pub units: Units,
    /// The language used for weather condition descriptions.
    ///
    /// Defaults to the API's own default (English) when unset.
    pub language: Option<String>,
    /// The location used when the `.w` command is invoked without arguments.
    pub default_location: Option<String>,
}

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

#[plugin(name = "openweathermap", settings = Settings)]
impl OpenWeatherMap {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let app_id = resolve_secret(settings.app_id.as_deref(), "OPENWEATHERMAP_APP_ID")?;
        let client = ctx.http().clone();

        Ok(Self {
            client,
            app_id,
            units: settings.units,
            language: settings.language.clone(),
            default_location: settings.default_location.clone(),
        })
    }

    /// Replies to `.w` with the current weather for the requested (or default) location.
    #[command(".w", "Show current weather for a location", event)]
    async fn weather(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let args = command.args();

        let location = if args.trim().is_empty() {
            let Some(location) = self.default_location.as_deref() else {
                send_reply(client, channel, plain_reply("Usage: .w\x0f <location>"))?;

                return Ok(());
            };

            location
        } else {
            args.trim()
        };

        let message = match self.fetch_weather(location).await {
            Ok(weather) => format_weather(&weather, self.units),
            Err(Error::LocationNotFound) => plain_reply("Location not found"),
            Err(e) => {
                warn!(exception.message = ?e, "openweathermap error");
                plain_reply(e)
            }
        };

        send_reply(client, channel, message)?;

        Ok(())
    }
}

/// Formats the weather response into a natural language string.
fn format_weather(w: &WeatherResponse, units: Units) -> String {
    let temp_unit = units.temperature();
    let speed_unit = units.wind_speed();

    // The reply is assembled in one buffer: the location, conditions, wind, and the extra
    // details are written as they are formatted instead of into intermediate strings that are
    // then joined and copied.
    let mut reply = String::with_capacity(PLAIN_REPLY_PREFIX.len() + 192);
    reply.push_str(PLAIN_REPLY_PREFIX);

    let _ = write!(reply, "Right now in {RESET}{}", irc::unstyle_text(&w.name));

    if let Some(country) = &w.sys.country {
        let _ = write!(reply, ", {}", irc::unstyle_text(country));
    }

    let _ = write!(
        reply,
        "{COLOR} it's {RESET}{:.1} {temp_unit}{COLOR} (feels like {RESET}{:.1} {temp_unit}{COLOR}) with {RESET}",
        w.main.temp, w.main.feels_like
    );

    if w.weather.is_empty() {
        reply.push_str("unknown conditions");
    } else {
        let conditions = w
            .weather
            .iter()
            .map(|description| irc::unstyle_text(&description.description))
            .oxford_join();

        reply.push_str(&conditions);
    }

    let _ = write!(
        reply,
        "{COLOR}. Wind: {RESET}{:.1} {speed_unit}",
        w.wind.speed
    );

    if let Some(gust) = w.wind.gust.filter(|gust| *gust > 0.0) {
        let _ = write!(reply, " (gusts: {gust:.1} {speed_unit})");
    }

    let _ = write!(
        reply,
        "{COLOR}. Humidity: {RESET}{}%{COLOR}. Pressure: {RESET}{} hPa{COLOR}",
        w.main.humidity, w.main.pressure
    );

    if let Some(clouds) = &w.clouds {
        let _ = write!(reply, ". Cloud coverage: {RESET}{}%{COLOR}", clouds.all);
    }

    reply
}

#[cfg(test)]
mod tests {
    use super::model::{Clouds, Main, Sys, WeatherDescription, Wind};
    use super::*;
    use zeta_test_support::settings_tests;

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
    fn formats_weather_matches_the_expected_layout() {
        assert_eq!(
            format_weather(&test_response(), Units::Metric),
            "\x0310> Right now in \x0fCopenhagen, DK\x0310 it's \x0f12.5 °C\x0310 \
             (feels like \x0f11.0 °C\x0310) with \x0flight rain\x0310. Wind: \x0f3.0 m/s \
             (gusts: 6.5 m/s)\x0310. Humidity: \x0f72%\x0310. Pressure: \x0f1013 hPa\x0310. \
             Cloud coverage: \x0f75%\x0310"
        );
    }

    #[test]
    fn formats_weather_in_both_unit_systems() {
        // The API converts the values itself, so formatting only swaps the labels: the same
        // fixture numbers are rendered with the selected unit names.
        let cases = [
            (
                "metric",
                Units::Metric,
                "12.5 °C",
                "3.0 m/s (gusts: 6.5 m/s)",
            ),
            ("imperial", Units::Imperial, "12.5 °F", "3.0 mph"),
        ];

        for (case, units, temperature, wind) in cases {
            let formatted = irc::strip_formatting(&format_weather(&test_response(), units));

            assert!(
                formatted.contains("Right now in Copenhagen, DK"),
                "{case}: {formatted}"
            );
            assert!(formatted.contains(temperature), "{case}: {formatted}");
            assert!(formatted.contains("feels like 11.0"), "{case}: {formatted}");
            assert!(formatted.contains(wind), "{case}: {formatted}");
            assert!(
                formatted.contains("with light rain. Wind:"),
                "{case}: {formatted}"
            );
        }
    }
}
