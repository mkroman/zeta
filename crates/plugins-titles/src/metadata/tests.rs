//! Tests for the page metadata extraction: the token sink, chunk decoding and the fixture.

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{BufferQueue, Tokenizer, TokenizerOpts};

use super::PageMetadata;
use super::fetch::decode_chunk;
use super::sink::HeadSink;

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
    let metadata = parse_metadata(r#"<head><meta property="og:description" content=" "> </head>"#);

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
