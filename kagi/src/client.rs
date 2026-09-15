use std::{sync::Arc, time::Instant};

use regex::Regex;
use secrecy::{ExposeSecret, SecretString};
use reqwest::header::{ACCEPT, SET_COOKIE};
use reqwest::redirect::Policy;
use scraper::{ElementRef, Html, Node, Selector};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, error};

use super::{BASE_URL, Error, HTTP_TIMEOUT, SESSION_DURATION, USER_AGENT, SearchResult};

/// Represents a message parsed from the Kagi socket stream.
/// The raw format is `Tag:JSON_BODY\0\n`.
#[derive(Deserialize, Debug)]
struct KagiMessage {
    /// The message tag (e.g., "search", "search.info", "meta").
    /// This is extracted from the wire prefix or the JSON body.
    pub tag: String,
    /// The flexible payload. Using `Value` allows this struct to handle
    /// diverse message types (HTML strings, objects, or nulls) without breaking.
    pub payload: Option<Value>,
    /// Optional version string sometimes found in the JSON body.
    #[allow(dead_code)]
    pub kagi_version: Option<String>,
}

#[derive(Clone, Debug)]
struct Session {
    /// The nonce used for the first search request.
    nonce: String,
    /// When the nonce was created.
    created_at: Instant,
}

/// Client for searching with Kagi.
pub struct Client {
    /// HTTP client with a cookie jar.
    http: reqwest::Client,
    /// Kagi login token.
    token: SecretString,
    /// Session details.
    session: Arc<RwLock<Option<Session>>>,
}

impl Session {
    fn is_valid(&self) -> bool {
        self.created_at.elapsed() < SESSION_DURATION
    }
}

impl Client {
    /// Constructs a new [`Client`] for searching with Kagi using the given session token.
    ///
    /// The token is the value of the `kagi_session` cookie from an authenticated browser session.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client fails to build.
    pub fn with_token(token: impl Into<SecretString>) -> Client {
        let client = reqwest::ClientBuilder::new()
            .cookie_store(true)
            .redirect(Policy::none())
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .expect("could not build http client");

        Client {
            http: client,
            token: token.into(),
            session: Arc::new(RwLock::new(None)),
        }
    }

    async fn fetch_new_session_data(&self) -> Result<String, Error> {
        // Issue a request with the login token to receive session cookies. The token is part of
        // the query string, so the request is intentionally not logged here.
        let req = self
            .http
            .get(format!("{BASE_URL}/search"))
            .query(&[("token", self.token.expose_secret())]);
        debug!("requesting session cookies");

        let res = req.send().await.map_err(Error::RequestSession)?;
        if !res.headers().contains_key(SET_COOKIE) {
            error!("the response does not include set-cookie headers!");
            return Err(Error::SessionCookies);
        }

        // Request the main page to receive a nonce for the first search.
        debug!("requesting nonce");
        let req = self.http.get(BASE_URL);
        let res = req.send().await.map_err(Error::RequestNonce)?;
        let body = res.text().await.map_err(Error::ReadNonce)?;

        extract_nonce(&body).ok_or(Error::Nonce)
    }

    async fn get_valid_nonce(&self) -> Result<String, Error> {
        // Optimistic read of a current session
        {
            let guard = self.session.read().await;

            if let Some(session) = guard.as_ref().filter(|s| s.is_valid()) {
                return Ok(session.nonce.clone());
            }
        }

        let new_nonce = {
            let mut guard = self.session.write().await;

            // Double check and return the current session if it was changed while we were waiting
            // for the lock.
            if let Some(session) = guard.as_ref().filter(|s| s.is_valid()) {
                return Ok(session.nonce.clone());
            }

            debug!("session expired or missing, refreshing...");
            let new_nonce = self.fetch_new_session_data().await?;

            *guard = Some(Session {
                nonce: new_nonce.clone(),
                created_at: Instant::now(),
            });

            new_nonce
        };

        Ok(new_nonce)
    }

    /// Searches Kagi with the given query and returns the parsed search results.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if a session could not be established, the search request could not
    /// be sent, or the response could not be read.
    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>, Error> {
        let _nonce = self.get_valid_nonce().await?;

        let req = self
            .http
            .get(format!("{BASE_URL}/socket/search"))
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

/// Parses a raw stream response body into a vector of `KagiMessage`s.
///
/// This handles the specific Kagi wire format:
/// 1. Splits by `\0\n` delimiter.
/// 2. Splits each chunk at the first `:` into (`WireTag`, `JsonBody`).
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
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/search_stream.bin");

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
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/landing.html");
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
