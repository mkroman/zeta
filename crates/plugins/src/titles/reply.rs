//! Formatting of page metadata for IRC replies.

use url::Url;
use zeta_utils::utils::{Truncatable, collapse_whitespace};

use super::Settings;
use super::metadata::PageMetadata;
use crate::prelude::*;

/// Formats the metadata of a page as an IRC message.
///
/// Returns [`None`] when the page has neither a title nor OpenGraph metadata worth posting.
#[must_use]
pub(super) fn format_page(
    metadata: &PageMetadata,
    url: &Url,
    settings: &Settings,
) -> Option<String> {
    let title = metadata
        .title
        .as_deref()
        .or(metadata.og_title.as_deref())
        .map(collapse_whitespace)
        .filter(|title| !title.is_empty());

    let description = metadata
        .description
        .as_deref()
        .map(collapse_whitespace)
        .filter(|description| !description.is_empty())
        .map(|description| description.truncate_within(settings.max_description_length, "…"));

    if title.is_none() && description.is_none() {
        return None;
    }

    let content = title
        .as_deref()
        .into_iter()
        .chain(description.as_deref())
        .collect::<Vec<_>>()
        .join(" — ");

    let message = if has_open_graph(metadata) {
        let site = metadata
            .site_name
            .as_deref()
            .map(collapse_whitespace)
            .filter(|site| !site.is_empty())
            .unwrap_or_else(|| host_name(url));

        reply(&site, &content)
    } else {
        plain_reply(&content)
    };

    Some(message.truncate_within(settings.max_message_length, "…"))
}

/// Whether any OpenGraph metadata was captured.
#[must_use]
const fn has_open_graph(metadata: &PageMetadata) -> bool {
    metadata.site_name.is_some() || metadata.og_title.is_some() || metadata.description.is_some()
}

/// Returns the host of `url` without its `www.` prefix.
#[must_use]
fn host_name(url: &Url) -> String {
    url.host_str()
        .map_or_default(|host| host.strip_prefix("www.").unwrap_or(host))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn falls_back_to_og_title_and_host() {
        let metadata = PageMetadata {
            og_title: Some("Hello".to_string()),
            description: Some("A description".to_string()),
            ..PageMetadata::default()
        };
        let url = Url::parse("https://www.maero.dk/page").unwrap();

        let message = format_page(&metadata, &url, &Settings::default()).unwrap();

        assert_eq!(
            message,
            "\x0310>\x0f\x02 maero.dk:\x02\x0310 Hello — A description"
        );
    }

    #[test]
    fn formats_description_only_pages() {
        let metadata = PageMetadata {
            site_name: Some("Maero".to_string()),
            description: Some("A description".to_string()),
            ..PageMetadata::default()
        };
        let url = Url::parse("https://maero.dk").unwrap();

        let message = format_page(&metadata, &url, &Settings::default()).unwrap();

        assert_eq!(message, "\x0310>\x0f\x02 Maero:\x02\x0310 A description");
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
