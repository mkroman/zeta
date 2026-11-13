//! The OpenWeatherMap API response and geocoding models.

use serde::Deserialize;

/// Result from the Geocoding API.
#[derive(Deserialize, Debug)]
pub struct GeocodingResult {
    /// Latitude.
    pub(super) lat: f64,
    /// Longitude.
    pub(super) lon: f64,
}

/// Result from the Current Weather Data API.
#[derive(Deserialize, Debug)]
pub struct WeatherResponse {
    /// City name.
    pub(super) name: String,
    /// System parameters.
    pub(super) sys: Sys,
    /// Main weather data (temperature, etc.).
    pub(super) main: Main,
    /// Weather condition descriptions.
    pub(super) weather: Vec<WeatherDescription>,
    /// Wind data.
    pub(super) wind: Wind,
    /// Cloud coverage.
    pub(super) clouds: Option<Clouds>,
}

/// System parameters (country, timestamps, etc.).
#[derive(Deserialize, Debug)]
pub struct Sys {
    /// Country code (e.g. "DK", "US").
    pub(super) country: Option<String>,
}

/// Main weather parameters.
#[derive(Deserialize, Debug)]
pub struct Main {
    /// Temperature in the configured unit.
    pub(super) temp: f64,
    /// Feels-like temperature in the configured unit.
    pub(super) feels_like: f64,
    /// Humidity percentage.
    pub(super) humidity: u8,
    /// Atmospheric pressure in hPa.
    pub(super) pressure: u16,
}

/// Weather condition description.
#[derive(Deserialize, Debug)]
pub struct WeatherDescription {
    /// Weather condition within the group.
    pub(super) description: String,
}

/// Wind statistics.
#[derive(Deserialize, Debug)]
pub struct Wind {
    /// Wind speed in the configured unit.
    pub(super) speed: f64,
    /// Wind gust in the configured unit.
    pub(super) gust: Option<f64>,
}

/// Cloud coverage.
#[derive(Deserialize, Debug)]
pub struct Clouds {
    /// Cloudiness, %.
    pub(super) all: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_api_response() {
        // The field names follow the API's recorded responses; extra fields are ignored.
        let response: WeatherResponse = serde_json::from_str(
            r#"{
                "name": "Copenhagen",
                "sys": {"country": "DK"},
                "main": {"temp": 12.5, "feels_like": 11.0, "humidity": 72, "pressure": 1013},
                "weather": [{"id": 500, "main": "Rain", "description": "light rain", "icon": "10d"}],
                "wind": {"speed": 3.0, "deg": 240, "gust": 6.5},
                "clouds": {"all": 75},
                "coord": {"lon": 12.56, "lat": 55.68},
                "dt": 1758600000
            }"#,
        )
        .expect("the api response should decode");

        assert_eq!(response.name, "Copenhagen");
        assert_eq!(response.sys.country.as_deref(), Some("DK"));
        assert!((response.main.temp - 12.5).abs() < f64::EPSILON);
        assert_eq!(response.weather[0].description, "light rain");
        assert_eq!(response.wind.gust, Some(6.5));
        assert_eq!(response.clouds.map(|clouds| clouds.all), Some(75));
    }
}
