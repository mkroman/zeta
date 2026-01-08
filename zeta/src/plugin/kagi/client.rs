use std::time::Instant;

use regex::Regex;
use reqwest::header::{ACCEPT, SET_COOKIE};
use scraper::{ElementRef, Html, Node, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error};

use crate::http;

use super::{Error, KAGI_SESSION_DURATION, SearchResult};

/// Represents a message parsed from the Kagi socket stream.
/// The raw format is `Tag:JSON_BODY\0\n`.
#[derive(Serialize, Deserialize, Debug)]
struct KagiMessage {
    /// The message tag (e.g., "search", "search.info", "meta").
    /// This is extracted from the wire prefix or the JSON body.
    pub tag: String,
    /// The flexible payload. Using `Value` allows this struct to handle
    /// diverse message types (HTML strings, objects, or nulls) without breaking.
    pub payload: Option<Value>,
    /// Optional version string sometimes found in the JSON body.
    pub kagi_version: Option<String>,
}

pub struct Client {
    /// HTTP client with a cookie jar.
    http: reqwest::Client,
    /// Kagi login token.
    token: String,
    /// The instant the current session started.
    session_started_at: Option<Instant>,
    /// The nonce used for the current session.
    nonce: Option<String>,
}

impl Client {
    pub fn with_token(token: String) -> Client {
        let client = http::client::builder()
            .cookie_store(true)
            .build()
            .expect("could not build http client");

        Client {
            http: client,
            token,
            nonce: None,
            session_started_at: None,
        }
    }

    pub async fn init_session(&mut self) -> Result<(), Error> {
        // Issue a request with the login token to receive session cookies.
        let req = self
            .http
            .get("https://kagi.com/search")
            .query(&[("token", &self.token)]);
        debug!(?req, "requesting session cookies");

        let res = req.send().await.map_err(Error::RequestSession)?;
        if !res.headers().contains_key(SET_COOKIE) {
            error!("the response does not include set-cookie headers!");

            return Err(Error::SessionCookies);
        }

        // Request the main page to receive a nonce for the first search.
        debug!("requesting nonce");
        let req = self.http.get("https://kagi.com/");
        let res = req.send().await.map_err(Error::RequestNonce)?;
        let body = res.text().await.map_err(Error::ReadNonce)?;

        match extract_nonce(&body) {
            Some(nonce) => {
                debug!(nonce, "started session");

                self.nonce = Some(nonce);
                self.session_started_at = Some(Instant::now());

                Ok(())
            }
            None => Err(Error::Nonce),
        }
    }

    pub async fn search(&mut self, query: &str) -> Result<Vec<SearchResult>, Error> {
        if let Some(instant) = self.session_started_at {
            if instant.elapsed() > KAGI_SESSION_DURATION {
                self.init_session().await?;
            }
        } else {
            self.init_session().await?;
        }

        let req = self
            .http
            .get("https://kagi.com/socket/search")
            .header(ACCEPT, "application/vnd.kagi.stream")
            .query(&[("q", query)]);
        debug!(?req, "searching for {query}");
        let res = req.send().await.map_err(|_| Error::SearchRequest)?;
        let body = res.text().await.map_err(|_| Error::SearchRequestBody)?;
        let stream_msgs = parse_kagi_stream(&body);
        let search_results = parse_search_result_messages(&stream_msgs);

        Ok(search_results)
    }
}

// Extracts the `window.sse_nonce` value from the raw HTML content.
fn extract_nonce(html: &str) -> Option<String> {
    let re = Regex::new(r#"window\.sse_nonce\s*=\s*"([^"]+)""#).ok()?;

    re.captures(html)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
}

/// Parses a raw stream response body into a vector of KagiMessages.
///
/// This handles the specific Kagi wire format:
/// 1. Splits by `\0\n` delimiter.
/// 2. Splits each chunk at the first `:` into (WireTag, JsonBody).
/// 3. Deserializes the JSON body.
/// 4. Ensures the `tag` field is populated.
fn parse_kagi_stream(raw_body: &str) -> Vec<KagiMessage> {
    raw_body
        .split("\0\n")
        .filter(|chunk| !chunk.is_empty())
        .filter_map(|chunk| {
            // Split wire format: "tag:json_data"
            let (wire_tag, json_str) = chunk.split_once(':')?;
            // Parse JSON body
            let mut message: KagiMessage = serde_json::from_str(json_str).ok()?;
            // Normalize Tag: If the JSON body didn't have a tag, use the wire tag.
            if message.tag.is_empty() {
                message.tag = wire_tag.to_string();
            }

            Some(message)
        })
        .collect()
}

fn parse_search_result_messages(messages: &[KagiMessage]) -> Vec<SearchResult> {
    let mut result: Vec<SearchResult> = vec![];
    let search_msgs = messages.iter().filter(|x| x.tag == "search");

    for msg in search_msgs {
        if let Some(content) = msg
            .payload
            .as_ref()
            .and_then(|p| p.get("content").and_then(|v| v.as_str()))
        {
            let mut results = parse_search_results_html(content);

            result.append(&mut results);
        }
    }

    result
}

fn parse_search_results_html(html: &str) -> Vec<SearchResult> {
    let fragment = Html::parse_fragment(html);
    let search_result_selector = Selector::parse("div.search-result").unwrap();
    let title_link_selector = Selector::parse("h3.__sri-title-box > a.__sri_title_link").unwrap();
    let description_selector = Selector::parse("div.__sri-desc > div").unwrap();

    let search_results = fragment.select(&search_result_selector);

    let mut results: Vec<SearchResult> = vec![];

    for result_div in search_results {
        let title = result_div.select(&title_link_selector).next();
        let description = result_div.select(&description_selector).next();

        if let (Some(title), Some(description)) = (title, description) {
            let url = title.attr("href").unwrap_or("").to_string();

            results.push(SearchResult {
                title: title.text().collect::<String>().trim().to_owned(),
                url,
                description: extract_topmost_text(&description),
            });
        }
    }

    results
}

fn extract_topmost_text(elem: &'_ ElementRef<'_>) -> String {
    let extracted_text: String = elem
        .children()
        .filter_map(|node| match node.value() {
            Node::Text(text_node) => {
                let text = text_node.trim();

                if text.is_empty() { None } else { Some(text) }
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");

    extracted_text
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn read_search_stream() -> String {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kagi/search_stream.bin");

        std::fs::read_to_string(path).expect("could not read search stream")
    }

    #[test]
    fn test_parse_stream() {
        let stream = read_search_stream();
        let result = parse_kagi_stream(&stream);

        assert_eq!(result.len(), 8); // 8 messages
    }

    #[test]
    fn test_extract_nonce() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kagi/landing.html");
        let html = std::fs::read_to_string(path).unwrap();
        let result = extract_nonce(&html).expect("could not extract nonce");

        assert_eq!(result, "f611c60f27d06eb15e6b542b5a2609cc");
    }

    #[test]
    fn test_search_results() {
        let stream = read_search_stream();
        let messages = parse_kagi_stream(&stream);
        let results = parse_search_result_messages(&messages);

        assert!(!results.is_empty());
        assert_eq!(results.len(), 19);

        let result = results.first().unwrap();

        assert_eq!(result.title, "Vitamin D - Health Professional Fact Sheet");
        assert_eq!(
            result.description,
            "Vitamin D (also referred to as calciferol) is a fat-soluble vitamin that is naturally present in a few foods, added to others, and available as a dietary ..."
        );
        assert_eq!(
            result.url,
            "https://ods.od.nih.gov/factsheets/VitaminD-HealthProfessional/"
        );
    }
}
