//! OpenGraph metadata and media details of Instagram pages.
//!
//! Media pages carry their author and caption in the `og:title` and `og:description` meta tags of
//! the document head. Instagram serves a login wall to requests it scores as bot traffic, in which
//! case the page carries no metadata — callers are expected to degrade gracefully.

use std::sync::OnceLock;

use scraper::{Html, Selector};
use tracing::debug;

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

/// The details of a piece of Instagram media, extracted from whichever source provided them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MediaDetails {
    /// The username of the media's author.
    pub author: Option<String>,
    /// The media's caption.
    pub caption: Option<String>,
}

impl MediaDetails {
    /// Extracts the details from the OpenGraph metadata of a media page.
    ///
    /// The `og:title` of a media page has the form `<author> on Instagram: "<caption>"`; when the
    /// title is generic — as it is on login walls — no details can be extracted. An empty caption
    /// falls back to the `og:description`.
    #[must_use]
    pub fn from_og(meta: &PageMetadata) -> Option<Self> {
        let title = meta
            .og_title
            .as_deref()
            .map(str::trim)
            .filter(|title| !is_generic_title(title))?;

        let (author, caption) = split_author_title(title);
        let caption = [Some(caption), meta.og_description.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|caption| !caption.is_empty());

        Some(Self {
            author: author.map(str::to_string),
            caption: caption.map(str::to_string),
        })
    }
}

/// Returns whether the title is the generic title of a login or placeholder page.
const fn is_generic_title(title: &str) -> bool {
    title.eq_ignore_ascii_case("Instagram") || title.eq_ignore_ascii_case("Login • Instagram")
}

/// Splits an `og:title` of the form `<author> on Instagram: "<caption>"` into its author and
/// caption, trimming the quotes wrapping the caption.
#[must_use]
fn split_author_title(title: &str) -> (Option<&str>, &str) {
    match title.split_once(" on Instagram: ") {
        Some((author, caption)) => (Some(author.trim()), caption.trim_matches('"').trim()),
        None => (None, title),
    }
}

/// The alphabet Instagram media shortcodes are encoded in: base64url.
const SHORTCODE_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Decodes the numeric media id a media shortcode encodes.
///
/// Shortcodes are the media id encoded in base64url; long shortcodes appended for private posts
/// decode to numbers beyond `u64`, so `u128` is used to leave room for shortcodes to keep growing.
///
/// Returns `None` when the shortcode is empty, carries a character outside the alphabet, or does
/// not fit a `u128`.
#[must_use]
pub fn shortcode_to_media_pk(shortcode: &str) -> Option<u128> {
    if shortcode.is_empty() {
        return None;
    }

    let mut pk = 0_u128;

    for byte in shortcode.bytes() {
        let digit = SHORTCODE_ALPHABET
            .iter()
            .position(|&c| c == byte)? as u128;

        pk = pk.checked_mul(64)?.checked_add(digit)?;
    }

    Some(pk)
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
/// itself even though it decompresses responses — and its absence is enough to get flagged. When
/// `session_cookie` is given, it is sent along, lifting the login wall for media that would
/// otherwise not be readable anonymously.
///
/// # Errors
///
/// Returns an error if the request fails or the server responds with an error status.
pub async fn fetch(
    client: &wreq::Client,
    url: &str,
    session_cookie: Option<&str>,
) -> Result<PageMetadata, Error> {
    debug!(%url, "fetching page metadata");

    let mut request = client.get(url);

    if let Some(session_cookie) = session_cookie {
        request = request.header(wreq::header::COOKIE, format!("sessionid={session_cookie}"));
    }

    let response = request.send().await?;

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

    #[test]
    fn test_details_from_og() {
        let details = MediaDetails::from_og(&extract(MEDIA_PAGE)).unwrap();

        assert_eq!(details.author.as_deref(), Some("user.name"));
        assert_eq!(details.caption.as_deref(), Some("Some caption & more"));

        // Login walls carry no details.
        assert_eq!(MediaDetails::from_og(&extract(LOGIN_PAGE)), None);

        // An empty caption falls back to the description.
        let metadata = PageMetadata {
            og_title: Some("user.name on Instagram: \"\"".to_string()),
            og_description: Some("description text".to_string()),
        };
        assert_eq!(
            MediaDetails::from_og(&metadata).unwrap().caption.as_deref(),
            Some("description text")
        );

        // A title without an author keeps its caption.
        let metadata = PageMetadata {
            og_title: Some("Just a title".to_string()),
            og_description: None,
        };
        let details = MediaDetails::from_og(&metadata).unwrap();
        assert_eq!(details.author, None);
        assert_eq!(details.caption.as_deref(), Some("Just a title"));
    }

    #[test]
    fn test_shortcode_to_media_pk() {
        // The media pk of the shortcode, as computed independently.
        assert_eq!(shortcode_to_media_pk("DdUGmgAifcq"), Some(3_986_840_604_117_694_250));
        assert_eq!(shortcode_to_media_pk("C7_Hlo8y9aP"), Some(3_386_458_817_721_783_951));
        assert_eq!(shortcode_to_media_pk(""), None);
        assert_eq!(shortcode_to_media_pk("ab;cd"), None);
        assert_eq!(shortcode_to_media_pk("ab.cd"), None);
    }
}
