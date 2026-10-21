//! Extraction of page title and OpenGraph metadata from a document head.

use std::cell::RefCell;

use html5ever::tokenizer::{EndTag, StartTag, Tag, Token, TokenSink, TokenSinkResult};

/// Page title and OpenGraph metadata extracted from a document head.
#[derive(Debug, Default)]
pub(super) struct PageMetadata {
    /// The contents of the first `<title>` element.
    pub(super) title: Option<String>,
    /// The `content` of the first `og:title` meta tag.
    pub(super) og_title: Option<String>,
    /// The `content` of the first `og:site_name` meta tag.
    pub(super) site_name: Option<String>,
    /// The `content` of the first `og:description` meta tag.
    pub(super) description: Option<String>,
}

/// Token sink that captures the title and OpenGraph metadata of a document head.
///
/// Tokenization is done — see [`HeadSink::is_done`] — once the head section has ended, either
/// through `</head>` or the start of `<body>`.
pub(super) struct HeadSink {
    /// The metadata captured so far.
    metadata: RefCell<PageMetadata>,
    /// The buffer for the contents of the current `<title>` element, when inside one.
    title: RefCell<Option<String>>,
    /// Whether the head section has ended and tokenization can stop.
    done: RefCell<bool>,
}

impl HeadSink {
    /// Constructs a new, empty sink.
    pub(super) fn new() -> Self {
        Self {
            metadata: RefCell::new(PageMetadata::default()),
            title: RefCell::new(None),
            done: RefCell::new(false),
        }
    }

    /// Whether the head section has ended and tokenization can stop.
    pub(super) fn is_done(&self) -> bool {
        *self.done.borrow()
    }

    /// Returns the metadata captured so far.
    pub(super) fn into_metadata(self) -> PageMetadata {
        self.metadata.into_inner()
    }

    /// Processes a start tag.
    fn start_tag(&self, tag: &Tag) {
        match &*tag.name {
            "title" => {
                let mut title = self.title.borrow_mut();

                // Only the first title element is captured.
                if title.is_none() && self.metadata.borrow().title.is_none() {
                    *title = Some(String::new());
                }
            }
            "meta" => self.capture_meta(tag),
            "body" => {
                self.done.replace(true);
            }
            _ => {}
        }
    }

    /// Processes an end tag.
    fn end_tag(&self, tag: &Tag) {
        match &*tag.name {
            "title" => {
                if let Some(title) = self.title.borrow_mut().take()
                    && !title.is_empty()
                {
                    self.metadata.borrow_mut().title = Some(title);
                }
            }
            "head" => {
                self.done.replace(true);
            }
            _ => {}
        }
    }

    /// Captures the value of the first `og:title`, `og:site_name`, or `og:description` meta tag.
    fn capture_meta(&self, tag: &Tag) {
        let Some(key) = tag_attr(tag, "property").or_else(|| tag_attr(tag, "name")) else {
            return;
        };

        let Some(content) = tag_attr(tag, "content").map(str::trim) else {
            return;
        };

        if content.is_empty() {
            return;
        }

        let mut metadata = self.metadata.borrow_mut();
        let slot = match key {
            "og:title" => &mut metadata.og_title,
            "og:site_name" => &mut metadata.site_name,
            "og:description" => &mut metadata.description,
            _ => return,
        };

        if slot.is_none() {
            *slot = Some(content.to_string());
        }
    }
}

impl TokenSink for HeadSink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<Self::Handle> {
        if self.is_done() {
            return TokenSinkResult::Continue;
        }

        match token {
            Token::TagToken(tag) => match tag.kind {
                StartTag => self.start_tag(&tag),
                EndTag => self.end_tag(&tag),
            },
            Token::CharacterTokens(text) => {
                if let Some(title) = self.title.borrow_mut().as_mut() {
                    title.push_str(&text);
                }
            }
            _ => {}
        }

        TokenSinkResult::Continue
    }
}

/// Returns the value of the named attribute of a tag.
#[must_use]
fn tag_attr<'a>(tag: &'a Tag, name: &str) -> Option<&'a str> {
    tag.attrs
        .iter()
        .find(|attribute| &*attribute.name.local == name)
        .map(|attribute| &*attribute.value)
}

#[cfg(test)]
mod tests {
    use html5ever::tendril::StrTendril;
    use html5ever::tokenizer::{BufferQueue, Tokenizer, TokenizerOpts};

    use super::*;

    /// Tokenizes `html` in a single chunk and returns the captured metadata.
    fn parse_metadata(html: &str) -> PageMetadata {
        parse_metadata_chunked(html, usize::MAX)
    }

    /// Tokenizes `html` in chunks of `chunk_size` characters and returns the captured metadata.
    fn parse_metadata_chunked(html: &str, chunk_size: usize) -> PageMetadata {
        let tokenizer = Tokenizer::new(HeadSink::new(), TokenizerOpts::default());
        let queue = BufferQueue::default();
        let chars: Vec<char> = html.chars().collect();

        for chunk in chars.chunks(chunk_size.max(1)) {
            let text: String = chunk.iter().collect();
            queue.push_back(StrTendril::from_slice(&text));
            let _ = tokenizer.feed(&queue);
        }

        tokenizer.sink.into_metadata()
    }

    #[test]
    fn extracts_title() {
        let metadata = parse_metadata(
            "<!DOCTYPE html><html><head><title>Hello world</title></head><body></body></html>",
        );

        assert_eq!(metadata.title.as_deref(), Some("Hello world"));
    }

    #[test]
    fn extracts_open_graph_metadata() {
        let metadata = parse_metadata(
            r#"<head><meta property="og:site_name" content="Maero">
            <meta property="og:title" content="Hello">
            <meta property="og:description" content="A description">
            <title>Ignored</title></head>"#,
        );

        assert_eq!(metadata.site_name.as_deref(), Some("Maero"));
        assert_eq!(metadata.og_title.as_deref(), Some("Hello"));
        assert_eq!(metadata.description.as_deref(), Some("A description"));
        assert_eq!(metadata.title.as_deref(), Some("Ignored"));
    }

    #[test]
    fn extracts_open_graph_metadata_by_name() {
        let metadata =
            parse_metadata(r#"<head><meta name="og:description" content="A description"></head>"#);

        assert_eq!(metadata.description.as_deref(), Some("A description"));
    }

    #[test]
    fn keeps_only_the_first_of_each_open_graph_field() {
        let metadata = parse_metadata(
            r#"<head><meta property="og:title" content="First">
            <meta property="og:title" content="Second"></head>"#,
        );

        assert_eq!(metadata.og_title.as_deref(), Some("First"));
    }

    #[test]
    fn ignores_empty_open_graph_content() {
        let metadata =
            parse_metadata(r#"<head><meta property="og:description" content=" "> </head>"#);

        assert!(metadata.description.is_none());
    }

    #[test]
    fn decodes_entities() {
        let metadata = parse_metadata(
            r#"<head><title>Fish &amp; Chips</title>
            <meta property="og:description" content="a &quot;quoted&quot; word"></head>"#,
        );

        assert_eq!(metadata.title.as_deref(), Some("Fish & Chips"));
        assert_eq!(metadata.description.as_deref(), Some("a \"quoted\" word"));
    }

    #[test]
    fn captures_only_the_first_title() {
        let metadata = parse_metadata(
            "<html><head><title>First</title></head><body><svg><title>Second</title></svg></body></html>",
        );

        assert_eq!(metadata.title.as_deref(), Some("First"));
    }

    #[test]
    fn stops_at_the_end_of_head() {
        let metadata = parse_metadata(
            "<html><head><title>Hello</title></head><body><title>Ignored</title></body></html>",
        );

        assert_eq!(metadata.title.as_deref(), Some("Hello"));
    }

    #[test]
    fn stops_at_the_start_of_body() {
        let metadata = parse_metadata(
            "<html><head><title>Hello</title><body><title>Ignored</title></body></html>",
        );

        assert_eq!(metadata.title.as_deref(), Some("Hello"));
    }

    #[test]
    fn captures_title_across_chunks() {
        let html = r#"<head><title>Fish &amp; Chips</title><meta property="og:description" content="A description"></head>"#;

        for chunk_size in [1, 2, 3, 5, 7, 11, 64] {
            let metadata = parse_metadata_chunked(html, chunk_size);

            assert_eq!(metadata.title.as_deref(), Some("Fish & Chips"));
            assert_eq!(
                metadata.description.as_deref(),
                Some("A description"),
                "chunk size {chunk_size}"
            );
        }
    }

    #[test]
    fn parses_the_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/titles/page.html");
        let html = std::fs::read_to_string(path).expect("could not read fixture");

        let metadata = parse_metadata(&html);

        assert_eq!(metadata.title.as_deref(), Some("Maero — Notes"));
        assert_eq!(metadata.site_name.as_deref(), Some("Maero"));
        assert_eq!(
            metadata.og_title.as_deref(),
            Some("Notes from the \"field\"")
        );
        assert_eq!(metadata.description.as_deref(), Some("A & B"));

        let chunked = parse_metadata_chunked(&html, 7);
        assert_eq!(chunked.title, metadata.title);
        assert_eq!(chunked.site_name, metadata.site_name);
        assert_eq!(chunked.og_title, metadata.og_title);
        assert_eq!(chunked.description, metadata.description);
    }
}
