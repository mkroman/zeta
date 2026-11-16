//! Reply formatting.
//!
//! Renders captured page metadata as a single IRC reply line, labeled with the site name (or the
//! host) when OpenGraph metadata is present and truncated to the configured limits.

use url::Url;

use zeta_plugin::irc;
use zeta_plugin::irc::{LABELED_PREFIX_BYTES, PLAIN_REPLY_PREFIX, write_labeled_reply_prefix};
use zeta_utils::text::TruncatableExt;

use crate::Settings;
use crate::metadata::PageMetadata;

/// The separator between a page's title and its description.
const CONTENT_SEPARATOR: &str = " — ";

/// Formats the metadata of a page as an IRC message.
///
/// Returns [`None`] when the page has neither a title nor OpenGraph metadata worth posting.
#[must_use]
pub(crate) fn format_page(
    metadata: &PageMetadata,
    url: &Url,
    settings: &Settings,
) -> Option<String> {
    let title = metadata
        .title
        .as_deref()
        .or(metadata.og_title.as_deref())
        .map(irc::unstyle_text)
        .filter(|title| !title.is_empty());

    let description = metadata
        .description
        .as_deref()
        .map(irc::unstyle_text)
        .filter(|description| !description.is_empty());

    if title.is_none() && description.is_none() {
        return None;
    }

    // `truncate_within` borrows the description when it already fits, so it is copied into
    // `content` directly instead of through an intermediate, truncated `String`.
    let description = description
        .as_deref()
        .map(|description| description.truncate_within(settings.max_description_length, "…"));

    let title = title.as_deref().unwrap_or_default();
    let description = description.as_deref().unwrap_or_default();

    let separator = (!title.is_empty() && !description.is_empty()).then_some(CONTENT_SEPARATOR);

    let capacity = title.len() + description.len() + separator.map_or(0, str::len);

    // The reply is one buffer: the prefix is written first and the content follows, so the
    // finished line never has to be copied into a second string.
    let mut message = if has_open_graph(metadata) {
        let site = metadata
            .site_name
            .as_deref()
            .map(irc::unstyle_text)
            .filter(|site| !site.is_empty());
        let label = site.as_deref().unwrap_or_else(|| host_name(url));

        let mut message = String::with_capacity(LABELED_PREFIX_BYTES + label.len() + capacity);
        // Written into the reply buffer instead of into a temporary string.
        write_labeled_reply_prefix(&mut message, label);

        message
    } else {
        let mut message = String::with_capacity(PLAIN_REPLY_PREFIX.len() + capacity);
        message.push_str(PLAIN_REPLY_PREFIX);

        message
    };

    message.push_str(title);

    if let Some(separator) = separator {
        message.push_str(separator);
    }

    message.push_str(description);

    // `truncate_within` borrows the message when it already fits, so the byte-length check
    // lets the common short reply skip the `Cow` clone its result would otherwise take.
    if message.len() <= settings.max_message_length {
        return Some(message);
    }

    Some(
        message
            .truncate_within(settings.max_message_length, "…")
            .into_owned(),
    )
}

/// Whether any OpenGraph metadata was captured.
#[must_use]
const fn has_open_graph(metadata: &PageMetadata) -> bool {
    metadata.site_name.is_some() || metadata.og_title.is_some() || metadata.description.is_some()
}

/// Returns the host of `url` without its `www.` prefix.
#[must_use]
fn host_name(url: &Url) -> &str {
    url.host_str().map_or("", zeta_url::strip_www)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_formatting_from_page_metadata() {
        let metadata = PageMetadata {
            title: Some("\x0310Fake\nTitle\x0f".to_string()),
            site_name: Some("Site\x02".to_string()),
            description: Some("first\nsecond".to_string()),
            ..PageMetadata::default()
        };
        let url = Url::parse("https://maero.dk").unwrap();

        let message =
            irc::strip_formatting(&format_page(&metadata, &url, &Settings::default()).unwrap());

        assert_eq!(message, "> Site: Fake Title — first second");
    }

    #[test]
    fn formats_plain_titles() {
        let metadata = PageMetadata {
            title: Some("Hello  world".to_string()),
            ..PageMetadata::default()
        };
        let url = Url::parse("https://maero.dk").unwrap();

        let message = format_page(&metadata, &url, &Settings::default()).unwrap();

        assert_eq!(message, "\x0310> Hello world");
    }

    #[test]
    fn formats_open_graph_pages() {
        let metadata = PageMetadata {
            title: Some("Hello".to_string()),
            site_name: Some("Maero".to_string()),
            description: Some("A description".to_string()),
            ..PageMetadata::default()
        };
        let url = Url::parse("https://maero.dk").unwrap();

        let message = format_page(&metadata, &url, &Settings::default()).unwrap();

        assert_eq!(
            message,
            "\x0310>\x0f\x02 Maero:\x02\x0310 Hello — A description"
        );
    }

    #[test]
    fn formats_open_graph_fallbacks() {
        let cases = [
            (
                // Without a site name, the host stands in, minus `www.`.
                PageMetadata {
                    og_title: Some("Hello".to_string()),
                    description: Some("A description".to_string()),
                    ..PageMetadata::default()
                },
                "https://www.maero.dk/page",
                "> maero.dk: Hello — A description",
            ),
            (
                // A description-only page still renders as an OpenGraph reply.
                PageMetadata {
                    site_name: Some("Maero".to_string()),
                    description: Some("A description".to_string()),
                    ..PageMetadata::default()
                },
                "https://maero.dk",
                "> Maero: A description",
            ),
        ];

        for (metadata, url, expected) in cases {
            let url = Url::parse(url).unwrap();
            let message = format_page(&metadata, &url, &Settings::default()).unwrap();

            assert_eq!(irc::strip_formatting(&message), expected, "for {url}");
        }
    }

    #[test]
    fn stays_silent_for_pages_without_metadata() {
        let url = Url::parse("https://maero.dk").unwrap();

        assert_eq!(
            format_page(&PageMetadata::default(), &url, &Settings::default()),
            None
        );
        assert_eq!(
            format_page(
                &PageMetadata {
                    title: Some("   ".to_string()),
                    ..PageMetadata::default()
                },
                &url,
                &Settings::default()
            ),
            None
        );
    }

    #[test]
    fn truncates_long_values() {
        let settings = Settings::default();
        let long = "x".repeat(settings.max_message_length + 1);
        let metadata = PageMetadata {
            title: Some(long),
            ..PageMetadata::default()
        };
        let url = Url::parse("https://maero.dk").unwrap();
        let message = format_page(&metadata, &url, &settings).unwrap();

        assert_eq!(message.chars().count(), settings.max_message_length);
        assert!(message.ends_with('…'));
    }
}
