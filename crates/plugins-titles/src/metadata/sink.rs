//! Push-based HTML token sink for the document head.
//!
//! Captures the title and OpenGraph metadata of the head as the tokenizer walks the document and
//! reports when the head has ended, so the tokenizer can stop before the rest of the response is
//! processed.

use std::cell::RefCell;

use html5ever::tokenizer::{EndTag, StartTag, Tag, Token, TokenSink, TokenSinkResult};

use super::PageMetadata;

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
