//! OpenGraph metadata of Instagram pages.
//!
//! Media pages carry their author and caption in the `og:title` and `og:description` meta tags of
//! the document head. Instagram serves a login wall to requests it scores as bot traffic, in which
//! case the page carries no metadata — callers are expected to degrade gracefully.

use std::sync::OnceLock;

use scraper::{Html, Selector};
use tracing::debug;
use wreq::header::ACCEPT_ENCODING;

/// The CSS selector for the `og:title` meta tag.
fn og_title_selector() -> &'static Selector {
    static SELECTOR: OnceLock<Selector> = OnceLock::new();

    SELECTOR.get_or_init(|| {
        Selector::parse(r#"meta[property="og:title"]"#).expect("a valid selector")
    })
}

/// The CSS selector for the `og:description` meta tag.
fn og_description_selector() -> &'static Selector {
    static SELECTOR: OnceLock<Selector> = OnceLock::new();

    SELECTOR.get_or_init(|| {
        Selector::parse(r#"meta[property="og:description"]"#).expect("a valid selector")
    })
}

/// The OpenGraph metadata of an Instagram page.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct PageMetadata {
    /// The `content` of the first `og:title` meta tag.
    pub og_title: Option<String>,
    /// The `content` of the first `og:description` meta tag.
    pub og_description: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The request failed, or the response body could not be read.
    #[error("request error: {0}")]
    Request(#[from] wreq::Error),
    /// The server responded with an unsuccessful status code.
    #[error("the page request failed with status {0}")]
    Status(wreq::StatusCode),
}

/// Fetches `url` and extracts its OpenGraph metadata.
///
/// `Accept-Encoding` must be set explicitly: like reqwest, wreq does not advertise the header
/// itself even though it decompresses responses — and its absence is enough to get flagged.
///
/// # Errors
///
/// Returns an error if the request fails or the server responds with an error status.
pub async fn fetch(client: &wreq::Client, url: &str) -> Result<PageMetadata, Error> {
    debug!(%url, "fetching page metadata");

    let response = client
        .get(url)
        .header(ACCEPT_ENCODING, "gzip, deflate, br, zstd")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(Error::Status(response.status()));
    }

    let body = response.text().await?;
    Ok(extract(&body))
}

/// Extracts the OpenGraph metadata from the body of an Instagram page.
#[must_use]
fn extract(body: &str) -> PageMetadata {
    let html = Html::parse_document(body);

    PageMetadata {
        og_title: meta_content(&html, og_title_selector()),
        og_description: meta_content(&html, og_description_selector()),
    }
}

/// Returns the `content` of the first `meta` tag matched by `selector`.
fn meta_content(html: &Html, selector: &Selector) -> Option<String> {
    let value = html.select(selector).next()?.value();
    let content = value.attr("content")?.trim();

    (!content.is_empty()).then(|| content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document head of an Instagram media page, with a caption carrying HTML entities.
    const MEDIA_PAGE: &str = r#"
        <!DOCTYPE html>
        <html>
        <head>
            <meta charset="utf-8" />
            <meta property="og:url" content="https://www.instagram.com/p/C7_Hlo8y9aP/" />
            <meta property="og:title" content="user.name on Instagram: &quot;Some caption &amp; more&quot;" />
            <meta property="og:description" content="1,234 likes, 56 comments - user.name on January 1, 2024" />
            <meta property="og:site_name" content="Instagram" />
        </head>
        <body></body>
        </html>
    "#;

    /// The login wall Instagram serves to requests it scores as bot traffic.
    const LOGIN_PAGE: &str = r#"
        <!DOCTYPE html>
        <html>
        <head>
            <meta charset="utf-8" />
            <title id="pageTitle">Instagram</title>
        </head>
        <body></body>
        </html>
    "#;

    #[test]
    fn test_extract_media_page() {
        let metadata = extract(MEDIA_PAGE);

        assert_eq!(
            metadata.og_title.as_deref(),
            Some("user.name on Instagram: \"Some caption & more\"")
        );
        assert_eq!(
            metadata.og_description.as_deref(),
            Some("1,234 likes, 56 comments - user.name on January 1, 2024")
        );
    }

    #[test]
    fn test_extract_login_page() {
        assert_eq!(extract(LOGIN_PAGE), PageMetadata::default());
    }

    #[test]
    fn test_extract_attribute_orders() {
        // The attribute order does not matter.
        let html = r#"<meta content="the content" property="og:title" />"#;
        assert_eq!(extract(html).og_title.as_deref(), Some("the content"));

        // Pages without a content attribute carry no metadata.
        let html = r#"<meta property="og:title" />"#;
        assert_eq!(extract(html), PageMetadata::default());
    }
}
