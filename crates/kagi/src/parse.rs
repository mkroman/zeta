//! Parsing of Kagi stream payloads and result HTML fragments.

use std::sync::OnceLock;

use htmlize::unescape;
use scraper::{ElementRef, Html, Node, Selector};
use serde::Deserialize;
use serde_json::Value;
use zeta_utils::text::quoted_string_literal;

use crate::{BASE_URL, ImageResult, SearchResult};

/// Represents a message parsed from a Kagi socket stream.
///
/// Messages arrive either framed as standard server-sent events or as `Tag:JSON_BODY\0\n`
/// chunks; in both cases each message carries a tag and a JSON payload.
#[derive(Deserialize, Debug)]
pub struct KagiMessage {
    /// The message tag (e.g., "search", "search.info", "meta").
    /// This is extracted from the wire prefix or the JSON body.
    tag: String,
    /// The flexible payload. Using `Value` allows this struct to handle
    /// diverse message types (HTML strings, objects, or nulls) without breaking.
    payload: Option<Value>,
}

/// The structured search results payload of a `search_results_json` message.
#[derive(Deserialize)]
struct SearchResultsJson {
    #[serde(default)]
    items: Vec<JsonSearchResult>,
}

/// A single structured result within a `search_results_json` payload.
#[derive(Deserialize)]
struct JsonSearchResult {
    title: String,
    url: String,
    #[serde(default)]
    snippet: String,
}

/// Builds a Kagi URL with the given query parameters, percent-encoding as needed.
///
/// # Panics
///
/// Panics if the static [`BASE_URL`] is not a valid URL.
pub fn url_with_query(path: &str, params: &[(&str, &str)]) -> reqwest::Url {
    reqwest::Url::parse_with_params(&format!("{BASE_URL}{path}"), params)
        .expect("the static base url always produces a valid url")
}

/// Extracts the `window.sse_nonce` value from the raw HTML content.
pub fn extract_nonce(html: &str) -> Option<String> {
    const MARKER: &str = "window.sse_nonce";

    html.match_indices(MARKER).find_map(|(index, _)| {
        let tail = &html[index + MARKER.len()..];
        let tail = tail.trim_start().strip_prefix('=')?.trim_start();
        let (literal, _) = quoted_string_literal(tail, '"')?;
        let nonce = literal.strip_circumfix('"', '"')?;

        if nonce.is_empty() {
            None
        } else {
            Some(nonce.to_owned())
        }
    })
}

/// Parses a raw stream response body into a vector of `KagiMessage`s, detecting whether the
/// server responded with the standard server-sent event framing or the legacy null-delimited
/// wire format.
pub fn parse_stream(raw_body: &str) -> Vec<KagiMessage> {
    if raw_body.lines().any(|line| line.starts_with("data:")) {
        parse_sse_stream(raw_body)
    } else {
        parse_kagi_stream(raw_body)
    }
}

/// Parses a standard server-sent event stream where each `data:` line carries a JSON array of
/// messages.
///
/// Any non-data lines (such as `id:` fields or the `hi` greeting) are ignored.
fn parse_sse_stream(raw_body: &str) -> Vec<KagiMessage> {
    raw_body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .flat_map(|json| serde_json::from_str::<Vec<KagiMessage>>(json).unwrap_or_default())
        .collect()
}

/// Parses a raw stream response body in the legacy `Tag:JSON_BODY\0\n` wire format into a vector
/// of `KagiMessage`s.
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

/// Returns the `content` string of a message's payload, if it has one.
fn payload_content(message: &KagiMessage) -> Option<&str> {
    message.payload.as_ref()?.get("content")?.as_str()
}

pub fn parse_search_result_messages(messages: &[KagiMessage]) -> Vec<SearchResult> {
    // Prefer the structured results payload when the server provides one.
    for message in messages
        .iter()
        .filter(|message| message.tag == "search_results_json")
    {
        let Some(payload) = message.payload.as_ref().and_then(Value::as_str) else {
            continue;
        };
        let Ok(results) = serde_json::from_str::<SearchResultsJson>(payload) else {
            continue;
        };

        if !results.items.is_empty() {
            return results
                .items
                .into_iter()
                .map(|item| SearchResult {
                    title: unescape(&item.title).into_owned(),
                    url: item.url,
                    description: unescape(&item.snippet).into_owned(),
                })
                .collect();
        }
    }

    // Fall back to parsing the HTML fragments of `search` messages.
    let mut result: Vec<SearchResult> = Vec::new();

    for content in messages
        .iter()
        .filter(|message| message.tag == "search")
        .filter_map(payload_content)
    {
        result.extend(parse_search_results_html(content));
    }

    result
}

/// The CSS selectors for parsing the search and image result fragments.
struct Selectors {
    /// A search result block.
    search_result: Selector,
    /// The title link of a search result block.
    title_link: Selector,
    /// The description of a search result block.
    description: Selector,
    /// An image result block.
    image_item: Selector,
    /// The thumbnail image of an image result block.
    image_thumbnail: Selector,
}

impl Selectors {
    /// Parses the result selectors.
    ///
    /// # Panics
    ///
    /// Panics if a selector is invalid; the selectors are compile-time constants.
    fn new() -> Self {
        Self {
            search_result: Selector::parse("div.search-result").expect("search result selector"),
            title_link: Selector::parse("h3.__sri-title-box > a.__sri_title_link")
                .expect("title link selector"),
            description: Selector::parse("div.__sri-desc > div").expect("description selector"),
            image_item: Selector::parse("div._0_img-results > div.item").expect("item selector"),
            image_thumbnail: Selector::parse("img._0_img_src").expect("thumbnail selector"),
        }
    }
}

/// Returns the pre-compiled result selectors.
fn selectors() -> &'static Selectors {
    static SELECTORS: OnceLock<Selectors> = OnceLock::new();

    SELECTORS.get_or_init(Selectors::new)
}

fn parse_search_results_html(html: &str) -> Vec<SearchResult> {
    let fragment = Html::parse_fragment(html);
    let selectors = selectors();

    let search_results = fragment.select(&selectors.search_result);

    let mut results = Vec::new();

    for result_div in search_results {
        let title = result_div.select(&selectors.title_link).next();
        let description = result_div.select(&selectors.description).next();

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

pub fn parse_image_result_messages(messages: &[KagiMessage]) -> Vec<ImageResult> {
    let mut result: Vec<ImageResult> = Vec::new();

    for content in messages
        .iter()
        .filter(|message| message.tag == "images")
        .filter_map(payload_content)
    {
        result.extend(parse_image_results_html(content));
    }

    result
}

fn parse_image_results_html(html: &str) -> Vec<ImageResult> {
    let fragment = Html::parse_fragment(html);
    let selectors = selectors();

    let mut results = Vec::new();

    for item in fragment.select(&selectors.image_item) {
        let value = item.value();
        let (Some(title), Some(page_url), Some(image_url)) = (
            value.attr("data-title"),
            value.attr("data-host_url"),
            value.attr("data-content_url"),
        ) else {
            continue;
        };
        let Some(thumbnail) = item
            .select(&selectors.image_thumbnail)
            .next()
            .and_then(|img| img.attr("src"))
        else {
            continue;
        };

        results.push(ImageResult {
            title: unescape(title).into_owned(),
            page_url: page_url.to_string(),
            image_url: image_url.to_string(),
            thumbnail_url: thumbnail.to_string(),
            width: value
                .attr("data-width")
                .and_then(|width| width.parse().ok())
                .unwrap_or(0),
            height: value
                .attr("data-height")
                .and_then(|height| height.parse().ok())
                .unwrap_or(0),
            host: value.attr("data-host").unwrap_or_default().to_string(),
            rank: value
                .attr("data-rank")
                .and_then(|rank| rank.parse().ok())
                .unwrap_or(0),
        });
    }

    results
}

fn extract_topmost_text(elem: &ElementRef<'_>) -> String {
    let mut extracted = String::new();

    for node in elem.children() {
        let Node::Text(text_node) = node.value() else {
            continue;
        };
        let text = text_node.trim();

        if text.is_empty() {
            continue;
        }

        if !extracted.is_empty() {
            extracted.push(' ');
        }

        extracted.push_str(text);
    }

    extracted
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn read_fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);

        String::from_utf8(std::fs::read(path).expect("could not read fixture"))
            .expect("fixture is not valid UTF-8")
    }

    #[test]
    fn test_parse_legacy_stream() {
        let stream = read_fixture("search_stream.bin");
        let result = parse_kagi_stream(&stream);

        assert_eq!(result.len(), 8); // 8 messages
    }

    #[test]
    fn test_parse_sse_stream() {
        let stream = read_fixture("search_stream_sse.bin");
        let result = parse_stream(&stream);

        assert!(
            result
                .iter()
                .any(|message| message.tag == "search_results_json")
        );
        assert!(result.iter().any(|message| message.tag == "search"));
        assert!(result.iter().any(|message| message.tag == "search.info"));
    }

    #[test]
    fn test_extract_nonce() {
        let html = read_fixture("landing.html");
        let result = extract_nonce(&html).expect("could not extract nonce");

        assert_eq!(result, "0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn test_extract_nonce_variants() {
        let html = r#"
            window.sse_nonce.other = "ignored";
            window.sse_nonce
                =
                "abc123";
        "#;

        assert_eq!(extract_nonce(html).as_deref(), Some("abc123"));
        assert_eq!(extract_nonce("no nonce here"), None);
        assert_eq!(extract_nonce("window.sse_nonce = \"\";"), None);
        assert_eq!(extract_nonce("window.sse_nonce = 42;"), None);
        assert_eq!(extract_nonce("window.sse_nonce = \"unterminated;"), None);
    }

    #[test]
    fn test_search_results_structured() {
        let stream = read_fixture("search_stream_sse.bin");
        let messages = parse_stream(&stream);
        let results = parse_search_result_messages(&messages);

        assert_ne!(results, Vec::new());

        let result = results.first().unwrap();

        assert_eq!(result.title, "Hello, world - Wikipedia");
        assert_eq!(result.url, "https://en.wikipedia.org/wiki/Hello,_world");
        // Snippets arrive HTML-entity encoded and must be decoded.
        assert!(result.description.contains('"'));
        assert!(!result.description.contains("&quot;"));
    }

    #[test]
    fn test_search_results_structured_unescapes_title_and_description() {
        let payload = r#"{"items":[{"title":"Fish &amp; Chips","url":"https://example.com/fish","snippet":"A &quot;classic&quot; dish &amp; more"}]}"#;
        let messages = vec![KagiMessage {
            tag: "search_results_json".to_string(),
            payload: Some(Value::String(payload.to_string())),
        }];

        let results = parse_search_result_messages(&messages);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Fish & Chips");
        assert_eq!(results[0].description, "A \"classic\" dish & more");
    }

    #[test]
    fn test_search_results_html_fallback() {
        let stream = read_fixture("search_stream.bin");
        let messages = parse_kagi_stream(&stream);
        let results = parse_search_result_messages(&messages);

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

    #[test]
    fn test_image_results() {
        let stream = read_fixture("images_stream_sse.bin");
        let messages = parse_stream(&stream);
        let results = parse_image_result_messages(&messages);

        assert_eq!(results.len(), 5);

        let result = results.first().unwrap();

        assert_eq!(
            result.title,
            "How to Do a Reverse Image Search From Your Phone"
        );
        assert_eq!(
            result.page_url,
            "https://www.entrepreneur.com/business-news/how-to-do-a-reverse-image-search-from-your-phone/297541"
        );
        assert_eq!(
            result.image_url,
            "https://assets.entrepreneur.com/images/misc/1500561136_1.jpg"
        );
        assert!(
            result
                .thumbnail_url
                .starts_with("https://p.kagi.com/proxy/")
        );
        assert_eq!(result.width, 740);
        assert_eq!(result.height, 475);
        assert_eq!(result.host, "www.entrepreneur.com");
        assert_eq!(result.rank, 0);
    }
}
