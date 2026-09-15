//! URL titles and OpenGraph metadata.
//!
//! Monitors messages for HTTP(s) URLs — including the `ttp`/`ttps` variants that are missing
//! their leading `h` — fetches each page, and posts its title and OpenGraph metadata to the
//! channel. Responses are streamed through a push-based HTML tokenizer and the download is
//! aborted as soon as the document head has been received.
//!
//! Response bodies are decoded as UTF-8; byte sequences that are not valid UTF-8 — including
//! entire pages served in another encoding, e.g. ISO-8859-1 — are decoded lossily.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::mpsc;

use futures::StreamExt;
use html5ever::tokenizer::{
    BufferQueue, EndTag, StartTag, Tag, Token, TokenSink, TokenSinkResult, Tokenizer,
    TokenizerOpts,
};
use html5ever::tendril::StrTendril;
use irc::client::Client;
use irc::proto::Command;
use serde::{Deserialize, Serialize};
use wreq::StatusCode;
use wreq::header::{ACCEPT_ENCODING, HeaderMap, HeaderValue, USER_AGENT};
use wreq::redirect::Policy;
use wreq_util::Emulation;
use thiserror::Error;
use tracing::{debug, warn};
use url::Url;

use crate::plugin::prelude::*;
use crate::url::{ExtractedUrl, ExtractUrls, SchemeMap};

/// The accepted schemes: `http` and `https`, plus the `ttp` and `ttps` variants that are missing
/// their leading `h` — the latter are repaired and announced before the page is fetched.
const SCHEMES: SchemeMap = &[
    ("http", None),
    ("https", None),
    ("ttp", Some("http")),
    ("ttps", Some("https")),
];

/// The default maximum size of a response before we stop processing it.
const MAX_RESPONSE_SIZE: u64 = 2 * 1024 * 1024;

/// IRC formatting prefix for plain replies.
const REPLY_PREFIX: &str = "\x0310>";

/// IRC formatting prefix for OpenGraph replies, colored and followed by a bold site name.
const OG_REPLY_PREFIX: &str = "\x0310>\x0f\x02 ";

/// IRC formatting suffix closing the bold site name of an OpenGraph reply.
const OG_REPLY_SUFFIX: &str = ":\x02\x0310 ";

/// File extensions that we avoid requesting to save time and bandwidth.
const BINARY_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".bmp", ".gif", ".avi", ".mpg", ".flv", ".3gp", ".mp4", ".exe", ".msi",
    ".mp3", ".flac", ".tar", ".tar.gz", ".tar.bz2", ".zip",
];

/// Settings for the titles plugin, from its `[plugins.titles]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Hosts whose URLs are left to dedicated plugins.
    ///
    /// Removing a host makes this plugin preview its URLs, which may cause it to reply
    /// alongside the plugin that normally handles the host.
    #[serde(default = "default_ignored_hosts")]
    pub ignored_hosts: Vec<String>,
    /// The maximum number of redirects to follow.
    #[serde(default = "default_max_redirects")]
    pub max_redirects: usize,
    /// The maximum length of a posted message, in characters.
    #[serde(default = "default_max_message_length")]
    pub max_message_length: usize,
    /// The maximum length of a posted description, in characters.
    #[serde(default = "default_max_description_length")]
    pub max_description_length: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ignored_hosts: default_ignored_hosts(),
            max_redirects: default_max_redirects(),
            max_message_length: default_max_message_length(),
            max_description_length: default_max_description_length(),
        }
    }
}

/// Returns the default hosts that are handled by dedicated plugins.
fn default_ignored_hosts() -> Vec<String> {
    [
        "reddit.com",
        "redd.it",
        "www.reddit.com",
        "www.youtube.com",
        "youtube.com",
        "youtu.be",
        "vm.tiktok.com",
        "tiktok.com",
    ]
    .map(String::from)
    .to_vec()
}

/// Returns the default maximum number of redirects to follow.
const fn default_max_redirects() -> usize {
    3
}

/// Returns the default maximum length of a posted message, in characters.
const fn default_max_message_length() -> usize {
    400
}

/// Returns the default maximum length of a posted description, in characters.
const fn default_max_description_length() -> usize {
    200
}

/// Titles plugin.
///
/// Fetches pages posted as URLs in a channel and posts their title and OpenGraph metadata as
/// soon as the document head has been received.
///
/// Pages are fetched with a client that emulates a modern browser down to its TLS and HTTP/2
/// fingerprints — aggressively bot-protected sites reject plain HTTP clients regardless of the
/// request headers they carry.
pub struct Titles {
    /// The HTTP client used for fetching pages.
    client: wreq::Client,
    /// The plugin settings used when processing URLs.
    settings: Settings,
}

/// Page title and OpenGraph metadata extracted from a document head.
#[derive(Debug, Default)]
struct PageMetadata {
    /// The contents of the first `<title>` element.
    title: Option<String>,
    /// The `content` of the first `og:title` meta tag.
    og_title: Option<String>,
    /// The `content` of the first `og:site_name` meta tag.
    site_name: Option<String>,
    /// The `content` of the first `og:description` meta tag.
    description: Option<String>,
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
    fn new() -> HeadSink {
        HeadSink {
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
            },
            "meta" => self.capture_meta(tag),
            "body" => {
                self.done.replace(true);
            },
            _ => {},
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
            },
            "head" => {
                self.done.replace(true);
            },
            _ => {},
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
            },
            _ => {},
        }

        TokenSinkResult::Continue
    }
}

/// Errors that can occur while fetching a page.
#[derive(Debug, Error)]
enum Error {
    /// The server returned an unsuccessful response.
    #[error("{0}")]
    Status(StatusCode),
    /// The request failed.
    #[error("Error: {0}")]
    Request(#[from] wreq::Error),
    /// The tokenizer task failed.
    #[error("could not tokenize the response")]
    Task(#[from] tokio::task::JoinError),
}

/// Fetches `url` and extracts the metadata of its document head.
///
/// The response is streamed through a push-based HTML tokenizer; as soon as the head section has
/// been tokenized, processing stops and the rest of the response is aborted.
///
/// # Errors
///
/// Returns an error if the request fails or the server returns an unsuccessful response.
async fn fetch_metadata(client: &wreq::Client, url: &Url) -> Result<PageMetadata, Error> {
    let response = client.get(url.as_str()).send().await?;
    let status = response.status();

    if !status.is_success() {
        return Err(Error::Status(status));
    }

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
        warn!(%url, %error, "reader task failed");
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
            },
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
                    },
                    Some(invalid_len) => {
                        text.push('\u{fffd}');
                        pending.drain(..valid + invalid_len);
                    },
                }
            },
        }
    }

    text
}

/// Returns the request headers layered on top of the emulated browser profile.
///
/// `Accept-Encoding` must be set explicitly: like reqwest, wreq does not advertise the header
/// itself even though it decompresses responses — and its absence is enough to get flagged.
///
/// The profile's user agent is overridden with the bot's Firefox 151 user agent. This skews with
/// the emulated Firefox 142 fingerprint, and that is deliberate: the profile defaults to macOS —
/// which anti-bot systems score far more aggressively when requests originate from datacenter
/// networks — and the matching Linux Firefox 142 user agent is rejected by DataDome outright,
/// while Firefox 151 passes.
fn emulated_headers(user_agent: &str) -> Result<HeaderMap, ZetaError> {
    let mut headers = HeaderMap::new();

    headers.insert(
        ACCEPT_ENCODING,
        HeaderValue::from_static("gzip, deflate, br, zstd"),
    );
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(user_agent).map_err(plugin_err)?,
    );

    Ok(headers)
}

#[async_trait]
impl Plugin<Context> for Titles {
    fn new(ctx: &Context) -> Result<Self, ZetaError> {
        let settings = ctx.config.plugins.titles.settings.clone();
        let client = wreq::Client::builder()
            .emulation(Emulation::Firefox142)
            .default_headers(emulated_headers(&ctx.config.http.user_agent)?)
            .redirect(Policy::limited(settings.max_redirects))
            .timeout(ctx.config.http.timeout)
            .build()
            .map_err(plugin_err)?;

        Ok(Titles { client, settings })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "titles".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        let Command::PRIVMSG(ref channel, ref text) = message.command else {
            return Ok(());
        };

        if should_ignore(text) {
            return Ok(());
        }

        let mut seen = HashSet::new();

        for ExtractedUrl { url, repaired_from } in ExtractUrls::with_schemes(text, SCHEMES) {
            if seen.contains(&url) {
                continue;
            }

            seen.insert(url.clone());

            self.process_url(url, repaired_from, channel, client);
        }

        Ok(())
    }
}

impl Titles {
    /// Processes a single extracted URL: announces repaired schemes, and spawns a task that
    /// fetches the page and posts its metadata to the channel.
    fn process_url(
        &self,
        url: Url,
        repaired_from: Option<&'static str>,
        channel: &str,
        client: &Client,
    ) {
        if let Some(scheme) = repaired_from {
            debug!(%scheme, %url, "posting repaired url");

            if let Err(error) = client.send_privmsg(channel, format!("{REPLY_PREFIX} {url}")) {
                warn!(%error, "could not send repaired url");
            }
        }

        if is_ignored_host(&url, &self.settings.ignored_hosts) {
            debug!(%url, "skipping url handled by another plugin");

            return;
        }

        if is_binary_url(&url) {
            debug!(%url, "skipping url that looks like binary content");

            return;
        }

        let http_client = self.client.clone();
        let sender = client.sender();
        let channel = channel.to_string();
        let settings = self.settings.clone();

        tokio::spawn(async move {
            let metadata = match fetch_metadata(&http_client, &url).await {
                Ok(metadata) => metadata,
                Err(error) => {
                    warn!(%url, %error, "could not fetch page metadata");

                    if let Err(send_error) =
                        sender.send_privmsg(&channel, format!("{REPLY_PREFIX} {error}"))
                    {
                        warn!(%send_error, "could not send fetch error");
                    }

                    return;
                },
            };

            if let Some(message) = format_page(&metadata, &url, &settings)
                && let Err(error) = sender.send_privmsg(&channel, message)
            {
                warn!(%error, "could not send page metadata");
            }
        });
    }
}

/// Whether the message should be ignored: CTCP messages, command invocations, and messages that
/// look like replies from this or other bots, which could cause feedback loops.
#[must_use]
fn should_ignore(text: &str) -> bool {
    if text.starts_with('\x01') {
        return true;
    }

    // Messages that look like command invocations, e.g. `.gis`.
    if text.starts_with('.')
        && text
            .chars()
            .nth(1)
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return true;
    }

    text.starts_with(REPLY_PREFIX)
}

/// Whether the host of `url` is handled by another plugin.
#[must_use]
fn is_ignored_host(url: &Url, ignored_hosts: &[String]) -> bool {
    url.host_str()
        .is_some_and(|host| ignored_hosts.iter().any(|ignored| ignored == host))
}

/// Whether the path of `url` looks like binary content.
#[must_use]
fn is_binary_url(url: &Url) -> bool {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .is_some_and(|segment| {
            let segment = segment.to_ascii_lowercase();
            BINARY_EXTENSIONS
                .iter()
                .any(|extension| segment.ends_with(extension))
        })
}

/// Formats the metadata of a page as an IRC message.
///
/// Returns [`None`] when the page has neither a title nor OpenGraph metadata worth posting.
#[must_use]
fn format_page(metadata: &PageMetadata, url: &Url, settings: &Settings) -> Option<String> {
    let title = metadata
        .title
        .as_deref()
        .or(metadata.og_title.as_deref())
        .map(clean)
        .filter(|title| !title.is_empty());

    let description = metadata
        .description
        .as_deref()
        .map(clean)
        .filter(|description| !description.is_empty())
        .map(|description| truncate(&description, settings.max_description_length));

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
            .map(clean)
            .filter(|site| !site.is_empty())
            .unwrap_or_else(|| host_name(url));

        format!("{OG_REPLY_PREFIX}{site}{OG_REPLY_SUFFIX}{content}")
    } else {
        format!("{REPLY_PREFIX} {content}")
    };

    Some(truncate(&message, settings.max_message_length))
}

/// Whether any OpenGraph metadata was captured.
#[must_use]
const fn has_open_graph(metadata: &PageMetadata) -> bool {
    metadata.site_name.is_some() || metadata.og_title.is_some() || metadata.description.is_some()
}

/// Collapses runs of whitespace in `value` into single spaces and trims the result.
#[must_use]
fn clean(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncates `value` to at most `max` characters, appending an ellipsis when truncated.
#[must_use]
fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }

    let mut truncated: String = value.chars().take(max.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

/// Returns the host of `url` without its `www.` prefix.
#[must_use]
fn host_name(url: &Url) -> String {
    url.host_str()
        .map(|host| host.strip_prefix("www.").unwrap_or(host))
        .unwrap_or_default()
        .to_string()
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
    }

    #[test]
    fn extracts_open_graph_metadata_by_name() {
        let metadata = parse_metadata(
            r#"<head><meta name="og:description" content="A description"></head>"#,
        );

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
        assert_eq!(
            metadata.description.as_deref(),
            Some("a \"quoted\" word")
        );
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
    fn carries_split_utf8_sequences_across_chunks() {
        let bytes = "a—b".as_bytes(); // em-dash is three bytes
        let mut pending = Vec::new();

        assert_eq!(decode_chunk(&mut pending, &bytes[..2]), "a");
        assert_eq!(decode_chunk(&mut pending, &bytes[2..]), "—b");
        assert!(pending.is_empty());
    }

    #[test]
    fn replaces_invalid_sequences() {
        let mut pending = Vec::new();

        assert_eq!(decode_chunk(&mut pending, &[0xFF, b'a']), "\u{fffd}a");
        assert_eq!(decode_chunk(&mut pending, b"b"), "b");
        assert_eq!(
            decode_chunk(&mut pending, "æ".as_bytes()),
            "æ"
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn parses_the_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/titles/page.html");
        let html = std::fs::read_to_string(path).expect("could not read fixture");

        let metadata = parse_metadata(&html);

        assert_eq!(metadata.title.as_deref(), Some("Maero — Notes"));
        assert_eq!(metadata.site_name.as_deref(), Some("Maero"));
        assert_eq!(metadata.og_title.as_deref(), Some("Notes from the \"field\""));
        assert_eq!(metadata.description.as_deref(), Some("A & B"));

        let chunked = parse_metadata_chunked(&html, 7);
        assert_eq!(chunked.title, metadata.title);
        assert_eq!(chunked.site_name, metadata.site_name);
        assert_eq!(chunked.og_title, metadata.og_title);
        assert_eq!(chunked.description, metadata.description);
    }

    #[test]
    fn ignores_command_messages() {
        assert!(should_ignore(".gis https://maero.dk"));
        assert!(should_ignore(".ofn"));
        assert!(should_ignore(".help"));

        assert!(!should_ignore("a .dot in a sentence https://maero.dk"));
        assert!(!should_ignore("... https://maero.dk"));
    }

    #[test]
    fn ignores_ctcp_and_bot_replies() {
        assert!(should_ignore("\x01ACTION posts a link\x01"));
        assert!(should_ignore("\x0310> Some page"));
        assert!(should_ignore("\x0310>\x0f\x02 Site:\x02\x0310 Some page"));

        assert!(!should_ignore("check this out https://maero.dk"));
    }

    #[test]
    fn ignores_hosts_handled_by_other_plugins() {
        let settings = Settings::default();

        assert!(is_ignored_host(
            &Url::parse("https://www.reddit.com/r/rust").unwrap(),
            &settings.ignored_hosts
        ));
        assert!(is_ignored_host(
            &Url::parse("https://youtu.be/abc").unwrap(),
            &settings.ignored_hosts
        ));
        assert!(is_ignored_host(
            &Url::parse("https://vm.tiktok.com/abc").unwrap(),
            &settings.ignored_hosts
        ));

        assert!(!is_ignored_host(
            &Url::parse("https://maero.dk").unwrap(),
            &settings.ignored_hosts
        ));
    }

    #[test]
    fn ignores_binary_urls() {
        assert!(is_binary_url(&Url::parse("https://maero.dk/photo.JPG").unwrap()));
        assert!(is_binary_url(&Url::parse("https://maero.dk/archive.tar.gz").unwrap()));
        assert!(is_binary_url(&Url::parse("https://maero.dk/video.mp4?start=30").unwrap()));

        assert!(!is_binary_url(&Url::parse("https://maero.dk/page.html").unwrap()));
        assert!(!is_binary_url(&Url::parse("https://maero.dk").unwrap()));
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

        assert_eq!(message, "\x0310>\x0f\x02 Maero:\x02\x0310 Hello — A description");
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

        assert_eq!(message, "\x0310>\x0f\x02 maero.dk:\x02\x0310 Hello — A description");
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
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 4), "hel…");
        assert_eq!(truncate("hæłlo", 4), "hæł…");

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

    #[test]
    fn default_settings() {
        let settings = Settings::default();

        assert_eq!(settings.ignored_hosts.len(), 8);
        assert_eq!(settings.max_redirects, 3);
        assert_eq!(settings.max_message_length, 400);
        assert_eq!(settings.max_description_length, 200);
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "ignored_hosts": ["example.com"],
            "max_redirects": 1,
            "max_message_length": 100,
            "max_description_length": 50,
        }))
        .expect("could not deserialize settings");

        assert_eq!(settings.ignored_hosts, ["example.com"]);
        assert_eq!(settings.max_redirects, 1);
        assert_eq!(settings.max_message_length, 100);
        assert_eq!(settings.max_description_length, 50);
    }
}
