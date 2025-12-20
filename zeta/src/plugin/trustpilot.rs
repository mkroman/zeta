//! Trustpilot integration plugin.
//!
//! This plugin allows users to query Trustpilot for business scores and reviews via the `.tp` command.

use num_format::{Locale, ToFormattedString};
use serde::Deserialize;
use tracing::{debug, warn};

use crate::{http, plugin::prelude::*};

/// The base URL for the Trustpilot API.
const API_BASE_URL: &str = "https://api.trustpilot.com/v1";

/// Plugin for querying Trustpilot business scores.
pub struct Trustpilot {
    /// HTTP client for making API requests.
    client: reqwest::Client,
    /// Trustpilot API key.
    api_key: String,
    /// Command handler.
    command: ZetaCommand,
}

/// Represents a business unit response from the Trustpilot API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BusinessUnit {
    /// The display name of the business unit.
    display_name: String,
    /// Identifying information about the business.
    name: BusinessName,
    /// The trust score information.
    score: Score,
    /// Review statistics.
    number_of_reviews: NumberOfReviews,
}

/// Identifying name of the business.
#[derive(Debug, Deserialize)]
struct BusinessName {
    /// The identifying slug used in URLs (e.g., "example.com").
    identifying: String,
}

/// Trust score container.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Score {
    /// The aggregate trust score (typically 0-5 or 0-10 depending on API version).
    trust_score: f64,
}

/// Review count container.
#[derive(Debug, Deserialize)]
struct NumberOfReviews {
    /// Total number of reviews received.
    total: u64,
}

/// Errors that can occur during Trustpilot lookups.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// An error occurred while performing the HTTP request.
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    /// The requested business was not found.
    #[error("business not found")]
    NotFound,
}

#[async_trait]
impl Plugin for Trustpilot {
    fn new() -> Self {
        let api_key = std::env::var("TRUSTPILOT_API_KEY")
            .expect("missing TRUSTPILOT_API_KEY environment variable");
        let client = http::build_client();
        let command = ZetaCommand::new(".tp");

        Self {
            client,
            api_key,
            command,
        }
    }

    fn name() -> Name {
        Name::from("trustpilot")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(query) = self.command.parse(user_message)
        {
            if query.trim().is_empty() {
                client.send_privmsg(channel, "\x0310> Usage: .tp\x0f <domain>")?;
                return Ok(());
            }

            match self.search(query).await {
                Ok(business) => {
                    client.send_privmsg(channel, format_business(&business))?;
                }
                Err(Error::NotFound) => {
                    client.send_privmsg(channel, "\x0310> No results found")?;
                }
                Err(e) => {
                    warn!(error = ?e, "trustpilot error");
                    client.send_privmsg(channel, format!("\x0310> Error: {e}"))?;
                }
            }
        }
        Ok(())
    }
}

impl Trustpilot {
    /// Searches for a business unit by name.
    ///
    /// # Arguments
    ///
    /// * `query` - The name or domain of the business to search for.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the API returns a 404, or `Error::Request` for other HTTP errors.
    async fn search(&self, query: &str) -> Result<BusinessUnit, Error> {
        let url = format!("{API_BASE_URL}/business-units/find");
        let params = [("apikey", &self.api_key), ("name", &query.to_string())];

        debug!(%url, ?params, "searching trustpilot");

        let response = self.client.get(&url).query(&params).send().await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::NotFound);
        }

        let response = response.error_for_status()?;
        let business = response.json::<BusinessUnit>().await?;

        Ok(business)
    }
}

/// Formats a business unit into an IRC-friendly string.
fn format_business(b: &BusinessUnit) -> String {
    let score = b.score.trust_score;
    let reviews = b.number_of_reviews.total.to_formatted_string(&Locale::en);
    let url = format!("https://dk.trustpilot.com/review/{}", b.name.identifying);
    let name = &b.display_name;

    format!(
        "\x0310>\x0f\x02 Trustpilot\x02\x0310 (\x0f{name}\x0310): Score:\x0f {score}\x0310/\x0f5.0\x0310 Reviews:\x0f {reviews}\x0310 - {url}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_business() {
        let business = BusinessUnit {
            display_name: "Cool Company".to_string(),
            name: BusinessName {
                identifying: "coolcompany.com".to_string(),
            },
            score: Score { trust_score: 4.8 },
            number_of_reviews: NumberOfReviews { total: 12345 },
        };

        let formatted = format_business(&business);
        // Note: contains IRC color codes
        assert_eq!(
            formatted,
            "\x0310>\x0f\x02 Trustpilot\x02\x0310 (\x0fCool Company\x0310): Score:\x0f 4.8\x0310/\x0f5.0\x0310 Reviews:\x0f 12,345\x0310 - https://dk.trustpilot.com/review/coolcompany.com"
        );
    }

    #[test]
    fn test_deserialize_business_unit() {
        let json = r#"{
            "displayName": "Test Company",
            "name": {
                "identifying": "test.com"
            },
            "score": {
                "trustScore": 4.5
            },
            "numberOfReviews": {
                "total": 100
            }
        }"#;

        let business: BusinessUnit = serde_json::from_str(json).expect("failed to deserialize");

        assert_eq!(business.display_name, "Test Company");
        assert_eq!(business.name.identifying, "test.com");
        assert_eq!(business.score.trust_score, 4.5f64);
        assert_eq!(business.number_of_reviews.total, 100);
    }
}
