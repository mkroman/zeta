//! Google Image Search plugin.
//!
//! This plugin scrapes the Google Image Search results page to find image URLs
//! and descriptions based on user queries.

use serde::Deserialize;
use tracing::{debug, warn};

use crate::{http, plugin::prelude::*};

/// Google Image Search plugin structure.
pub struct GoogleImages {
    /// HTTP client for making requests.
    client: reqwest::Client,
    /// Command handler for the `.gis` command.
    command: ZetaCommand,
}

/// Errors that can occur during the execution of the Google Images plugin.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurred while performing the HTTP request.
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    /// An error occurred while parsing the JSON response.
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
    /// No results were found for the query.
    #[error("no results found")]
    NoResults,
    /// The response from Google was not in the expected format.
    #[error("invalid response format: could not find json start")]
    InvalidResponse,
}

/// The top-level response structure found in the Google HTML.
#[derive(Debug, Deserialize)]
struct SearchResponse {
    /// The container for image search metadata.
    ischj: Ischj,
}

/// Container for image metadata list.
#[derive(Debug, Deserialize)]
struct Ischj {
    /// List of image metadata.
    metadata: Vec<Metadata>,
}

/// Metadata for a single image result.
#[derive(Debug, Deserialize)]
struct Metadata {
    /// Text details associated with the image.
    text_in_grid: TextInGrid,
    /// Details about the original image source.
    original_image: OriginalImage,
}

/// Textual information about the image.
#[derive(Debug, Deserialize)]
struct TextInGrid {
    /// A text snippet describing the image.
    snippet: String,
}

/// Information about the original image.
#[derive(Debug, Deserialize)]
struct OriginalImage {
    /// The URL of the original image.
    url: String,
}

#[async_trait]
impl Plugin for GoogleImages {
    fn new() -> Self {
        let client = http::build_client();
        let command = ZetaCommand::new(".gis");

        Self { client, command }
    }

    fn name() -> Name {
        Name::from("google_images")
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
                client.send_privmsg(channel, "\x0310> Usage: .gis\x0f <query>")?;
                return Ok(());
            }

            match self.search(query).await {
                Ok(result) => {
                    let snippet = result.text_in_grid.snippet;
                    let url = result.original_image.url;
                    client.send_privmsg(
                        channel,
                        format!("\x0310>\x0f\x02 Google:\x02\x0310 {snippet} - {url}"),
                    )?;
                }
                Err(Error::NoResults) => {
                    client.send_privmsg(channel, "\x0310> No results")?;
                }
                Err(err) => {
                    warn!(?err, "google image search failed");
                    client.send_privmsg(channel, format!("\x0310> Error: {err}"))?;
                }
            }
        }

        Ok(())
    }
}

impl GoogleImages {
    /// Performs a search for images matching the given query.
    ///
    /// # Arguments
    ///
    /// * `query` - The search query string.
    ///
    /// # Returns
    ///
    /// Returns the metadata for the first image found, or an error.
    async fn search(&self, query: &str) -> Result<Metadata, Error> {
        debug!(%query, "searching for images");

        let params = [
            ("q", query),
            ("tbm", "isch"),
            ("asearch", "isch"),
            ("async", "_fmt:json,p:1,ijn:0"),
        ];

        let response = self
            .client
            .get("https://www.google.com/search")
            .query(&params)
            .header("Accept", "*/*")
            .send()
            .await?;

        let body = response.text().await?;

        Self::parse_response(&body)
    }

    /// Parses the HTML response body to extract image metadata.
    ///
    /// This method looks for a specific JSON structure (`{"ischj": ...`) embedded within
    /// the HTML response from Google.
    fn parse_response(body: &str) -> Result<Metadata, Error> {
        // The response body contains JSON but usually has a prefix or is embedded.
        // We look for the start of the `ischj` object as per the Ruby reference.
        let offset = body.find("{\"ischj\":").ok_or(Error::InvalidResponse)?;
        let json_part = &body[offset..];

        // Attempt to deserialize. Use a stream deserializer to ignore potential trailing garbage.
        let mut stream =
            serde_json::Deserializer::from_str(json_part).into_iter::<SearchResponse>();
        let search_response = stream.next().ok_or(Error::InvalidResponse)??;

        // Extract the first image metadata
        search_response
            .ischj
            .metadata
            .into_iter()
            .next()
            .ok_or(Error::NoResults)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_response_success() {
        let body = r#"
            some html junk
            <script>
            var meta = {"ischj":{"metadata":[{"text_in_grid":{"snippet":"A cute cat"},"original_image":{"url":"https://example.com/cat.jpg"}}]}};
            </script>
            more junk
        "#;

        let result = GoogleImages::parse_response(body).unwrap();
        assert_eq!(result.text_in_grid.snippet, "A cute cat");
        assert_eq!(result.original_image.url, "https://example.com/cat.jpg");
    }

    #[test]
    fn test_parse_response_no_json() {
        let body = "just some text without the magic key";
        let result = GoogleImages::parse_response(body);
        assert!(matches!(result, Err(Error::InvalidResponse)));
    }

    #[test]
    fn test_parse_response_empty_metadata() {
        let body = r#"{"ischj":{"metadata":[]}}"#;
        let result = GoogleImages::parse_response(body);
        assert!(matches!(result, Err(Error::NoResults)));
    }

    #[test]
    fn test_parse_response_malformed_json() {
        let body = r#"{"ischj":{"metadata": [ truncated"#;
        let result = GoogleImages::parse_response(body);
        assert!(matches!(result, Err(Error::Json(_))));
    }
}
