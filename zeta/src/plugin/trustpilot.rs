//! Looks up a business's Trustpilot score and review count.
//!
//! The `.tp <business>` command searches Trustpilot for the business and replies with its
//! name, star score (halved when the API reports a 0–10 scale), number of reviews, and a link
//! to its review page on the domain configured by `review_domain` (default `dk`). An empty
//! query replies with usage.
//!
//! The API key is set in `[plugins.trustpilot]`, falling back to the `TRUSTPILOT_API_KEY`
//! environment variable; a missing key fails plugin initialization and the plugin is skipped
//! at startup.

use num_format::{Locale, ToFormattedString};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{http, plugin::prelude::*};

/// The base URL for the Trustpilot API.
const API_BASE_URL: &str = "https://api.trustpilot.com/v1";

/// Settings for the trustpilot plugin, from its `[plugins.trustpilot]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The Trustpilot API key.
    ///
    /// Falls back to the `TRUSTPILOT_API_KEY` environment variable when unset.
    pub api_key: Option<String>,
    /// The Trustpilot domain used for review links (e.g. `dk`).
    pub review_domain: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_key: None,
            review_domain: "dk".to_string(),
        }
    }
}

/// The `.tp` command.
const TRUSTPILOT: CommandSpec = CommandSpec::new(".tp", "Look up a business's Trustpilot score");

/// Plugin for querying Trustpilot business scores.
pub struct Trustpilot {
    /// HTTP client for making API requests.
    client: reqwest::Client,
    /// Trustpilot API key.
    api_key: String,
    /// The Trustpilot domain used for review links.
    review_domain: String,
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
    /// The API returned an error, e.g. a non-success status or an unparseable body.
    #[error(transparent)]
    Api(#[from] http::ApiError),
    /// The requested business was not found.
    #[error("business not found")]
    NotFound,
}

#[async_trait]
impl Plugin<Context> for Trustpilot {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.command(TRUSTPILOT);
        let api_key = resolve_secret(settings.api_key.as_deref(), "TRUSTPILOT_API_KEY")?;
        let client = http::build_client(&ctx.config.http);

        Ok(Self {
            client,
            api_key,
            review_domain: settings.review_domain.clone(),
        })
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let query = command.args();

        reply_lookup(
            client,
            channel,
            query,
            &TRUSTPILOT.usage_line("<domain name>"),
            |query| async move { self.search(&query).await },
            |business| format_business(business, &self.review_domain),
        )
        .await?;

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
    /// Returns `Error::NotFound` if the API returns a 404, or another [`Error`] for other
    /// failures.
    async fn search(&self, query: &str) -> Result<BusinessUnit, Error> {
        let url = format!("{API_BASE_URL}/business-units/find");
        let params = [("name", &query.to_string())];

        debug!(%url, ?params, "searching trustpilot");

        let request = self
            .client
            .get(&url)
            .header("apikey", &self.api_key)
            .query(&params);
        http::get_json_or_404(request, Error::NotFound).await
    }
}

/// Formats a business unit into an IRC-friendly string.
fn format_business(b: &BusinessUnit, review_domain: &str) -> String {
    let score = normalized_score(b.score.trust_score);
    let reviews = b.number_of_reviews.total.to_formatted_string(&Locale::en);
    let url = format!(
        "https://{review_domain}.trustpilot.com/review/{}",
        b.name.identifying
    );
    let name = &b.display_name;

    reply(
        "Trustpilot",
        format!(
            "(\x0f{name}\x0310): Score:\x0f {score:.1}\x0310/\x0f5.0\x0310 Reviews:\x0f {reviews}\x0310 - {url}"
        ),
    )
}

/// Normalizes a trust score to the 0-5 scale.
///
/// Depending on the API version the score is on a 0-5 or 0-10 scale; scores above 5 are treated
/// as 0-10 and halved.
fn normalized_score(trust_score: f64) -> f64 {
    if trust_score > 5.0 {
        trust_score / 2.0
    } else {
        trust_score
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.api_key.is_none());
            assert_eq!(settings.review_domain, "dk");
        }
        deserialize: {
            "api_key": "secret",
            "review_domain": "www",
        } assert: {
            assert_eq!(settings.api_key.as_deref(), Some("secret"));
            assert_eq!(settings.review_domain, "www");
        }
    }

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

        let formatted = format_business(&business, "dk");
        // Note: contains IRC color codes
        assert_eq!(
            formatted,
            "\x0310>\x0f\x02 Trustpilot:\x02\x0310 (\x0fCool Company\x0310): Score:\x0f 4.8\x0310/\x0f5.0\x0310 Reviews:\x0f 12,345\x0310 - https://dk.trustpilot.com/review/coolcompany.com"
        );
    }

    #[test]
    fn normalizes_ten_point_scores() {
        let business = BusinessUnit {
            display_name: "Cool Company".to_string(),
            name: BusinessName {
                identifying: "coolcompany.com".to_string(),
            },
            score: Score { trust_score: 9.6 },
            number_of_reviews: NumberOfReviews { total: 1 },
        };

        let formatted = format_business(&business, "www");

        assert!(formatted.contains("Score:\x0f 4.8\x0310"), "{formatted}");
        assert!(
            formatted.contains("https://www.trustpilot.com/review/coolcompany.com"),
            "{formatted}"
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
        assert!((business.score.trust_score - 4.5).abs() < f64::EPSILON);
        assert_eq!(business.number_of_reviews.total, 100);
    }
}
