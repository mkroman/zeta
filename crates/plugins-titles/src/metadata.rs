//! Page metadata extraction.
//!
//! Fetches a page, streams its response through a push-based HTML tokenizer, and captures the
//! title and OpenGraph metadata of the document head. Tokenization stops as soon as the head has
//! ended, aborting the rest of the download.

use std::cell::RefCell;
use std::sync::mpsc;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, EndTag, StartTag, Tag, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use thiserror::Error;
use tracing::warn;
use url::Url;

use zeta_http as http;
use zeta_http::emulated::ResponseError;

/// The default maximum size of a response before we stop processing it.
const MAX_RESPONSE_SIZE: u64 = 2 * 1024 * 1024;

/// Page title and OpenGraph metadata extracted from a document head.
#[derive(Debug, Default)]
pub struct PageMetadata {
    /// The contents of the first `<title>` element.
    pub title: Option<String>,
    /// The `content` of the first `og:title` meta tag.
    pub og_title: Option<String>,
    /// The `content` of the first `og:site_name` meta tag.
    pub site_name: Option<String>,
    /// The `content` of the first `og:description` meta tag.
    pub description: Option<String>,
}

/// Token sink that captures the title and OpenGraph metadata of a document head.
///
/// Tokenization is done — see [`HeadSink::is_done`] — once the head section has ended, either
/// through `</head>` or the start of `<body>`.
struct HeadSink {
    /// The metadata captured so far.
    metadata: RefCell<PageMetadata>,
    /// The buffer for the contents of the current `<title>` element, when inside one.
    title: RefCell<Option<String>>,
    /// Whether the head section has ended and tokenization can stop.
    done: RefCell<bool>,
}

impl HeadSink {
    /// Constructs a new, empty sink.
    fn new() -> Self {
        Self {
            metadata: RefCell::new(PageMetadata::default()),
            title: RefCell::new(None),
            done: RefCell::new(false),
        }
    }

    /// Whether the head section has ended and tokenization can stop.
    fn is_done(&self) -> bool {
        *self.done.borrow()
    }

    /// Returns the metadata captured so far.
    fn into_metadata(self) -> PageMetadata {
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

/// Errors that can occur while fetching a page.
#[derive(Debug, Error)]
pub enum Error {
    /// The page request failed or returned an unsuccessful status.
    #[error(transparent)]
    Response(#[from] ResponseError),
    /// The tokenizer task failed.
    #[error("could not tokenize the response")]
    Task(#[from] tokio::task::JoinError),
}

impl From<wreq::Error> for Error {
    fn from(error: wreq::Error) -> Self {
        Self::Response(ResponseError::from(error))
    }
}

/// Fetches `url` and extracts the metadata of its document head.
///
/// The response is streamed through a push-based HTML tokenizer; as soon as the head section has
/// been tokenized, processing stops and the rest of the response is aborted.
///
/// # Errors
///
/// Returns an error if the request fails or the server returns an unsuccessful response.
pub async fn fetch_metadata(client: &wreq::Client, url: &Url) -> Result<PageMetadata, Error> {
    use futures::StreamExt;

    let response = http::emulated::send(client.get(url.as_str())).await?;

    // The HTML tokenizer is not `Send`, so the response body is streamed through a channel and
    // tokenized on a blocking thread. The channel is bounded so that the reader stalls — and the
    // download is throttled — once the tokenizer falls behind.
    let (sender, receiver) = mpsc::sync_channel::<Result<Vec<u8>, wreq::Error>>(4);

    let reader = tokio::spawn(async move {
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let sent = match chunk {
                Ok(chunk) => sender.send(Ok(chunk.to_vec())).is_ok(),
                Err(error) => sender.send(Err(error)).is_ok(),
            };

            if !sent {
                break;
            }
        }
    });

    let metadata = tokio::task::spawn_blocking(move || -> Result<PageMetadata, Error> {
        let tokenizer = Tokenizer::new(HeadSink::new(), TokenizerOpts::default());
        let queue = BufferQueue::default();
        let mut pending: Vec<u8> = Vec::new();
        let mut total = 0_u64;

        while let Ok(chunk) = receiver.recv() {
            // A stream error before the head has been tokenized fails the fetch. Errors after
            // that point are never observed — the tokenizer stops as soon as it is done.
            let chunk = chunk.map_err(Error::from)?;
            total += chunk.len() as u64;

            // Stop processing oversized responses and settle for the metadata collected so far.
            if total > MAX_RESPONSE_SIZE {
                break;
            }

            let text = decode_chunk(&mut pending, &chunk);
            queue.push_back(StrTendril::from_slice(&text));

            let _ = tokenizer.feed(&queue);

            if tokenizer.sink.is_done() {
                break;
            }
        }

        if !pending.is_empty() {
            let text = String::from_utf8_lossy(&pending);
            queue.push_back(StrTendril::from_slice(&text));
            let _ = tokenizer.feed(&queue);
        }

        Ok(tokenizer.sink.into_metadata())
    });

    let metadata = metadata.await??;

    // The reader finishes once the tokenizer stops reading from the channel, aborting the rest of
    // the response.
    if let Err(error) = reader.await {
        warn!(
            exception.message = %error,
            "reader task failed"
        );
    }

    Ok(metadata)
}

/// Converts `chunk` to UTF-8 text for the tokenizer, carrying incomplete multi-byte sequences in
/// `pending` over to the next chunk.
///
/// Invalid sequences are replaced with `U+FFFD`, as if the bytes were decoded lossily.
#[must_use]
fn decode_chunk(pending: &mut Vec<u8>, chunk: &[u8]) -> String {
    pending.extend_from_slice(chunk);
    let mut text = String::new();

    loop {
        match std::str::from_utf8(pending) {
            Ok(valid) => {
                text.push_str(valid);
                pending.clear();

                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();

                if valid > 0 {
                    // The prefix is valid UTF-8 by construction.
                    if let Ok(prefix) = std::str::from_utf8(&pending[..valid]) {
                        text.push_str(prefix);
                    }
                }

                match error.error_len() {
                    // An incomplete sequence at the end of the chunk is carried over as-is.
                    None => {
                        pending.drain(..valid);

                        break;
                    }
                    Some(invalid_len) => {
                        text.push('\u{fffd}');
                        pending.drain(..valid + invalid_len);
                    }
                }
            }
        }
    }

    text
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

        // The `name` attribute is accepted as an alternative to `property`.
        let by_name =
            parse_metadata(r#"<head><meta name="og:description" content="A description"></head>"#);

        assert_eq!(by_name.description.as_deref(), Some("A description"));
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
        let cases = [
            // A closing `</head>` stops capture.
            (
                "<html><head><title>Hello</title></head><body><title>Ignored</title></body></html>",
                "after </head>",
            ),
            // So does a `<body>` start.
            (
                "<html><head><title>Hello</title><body><title>Ignored</title></body></html>",
                "at <body>",
            ),
        ];

        for (html, label) in cases {
            let metadata = parse_metadata(html);
            assert_eq!(metadata.title.as_deref(), Some("Hello"), "{label}");
        }
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
    fn carries_split_utf8_sequences_across_chunks() {
        let bytes = "a—b".as_bytes(); // em-dash is three bytes
        let mut pending = Vec::new();

        assert_eq!(decode_chunk(&mut pending, &bytes[..2]), "a");
        assert_eq!(decode_chunk(&mut pending, &bytes[2..]), "—b");
        assert_eq!(pending, Vec::<u8>::new());
    }

    #[test]
    fn replaces_invalid_sequences() {
        let mut pending = Vec::new();

        assert_eq!(decode_chunk(&mut pending, &[0xFF, b'a']), "\u{fffd}a");
        assert_eq!(decode_chunk(&mut pending, b"b"), "b");
        assert_eq!(decode_chunk(&mut pending, "æ".as_bytes()), "æ");
        assert_eq!(pending, Vec::<u8>::new());
    }

    #[test]
    fn parses_the_fixture() {
        let html = zeta_test_support::fixture!("titles/page.html");

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
