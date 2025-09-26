//! A client and parser for the Danish Dictionary (*Den Danske Ordbog*).
//!
//! This crate provides tools to query the official Danish dictionary web service at `ordnet.dk`
//! and parse the resulting HTML into structured Rust types.
//!
//! ## Features
//!
//! - **`client`**: Enables the `async` [`Client`] for making live requests to the dictionary
//!   service. (Enabled by default).
//! - **`serde`**: Adds `Serialize` and `Deserialize` derives on all public data structures in the
//!   [`types`] module.
//!
//! ## Quick Start
//!
//! To get started, add this crate to your `Cargo.toml`. The main entry point for fetching data is
//! the [`Client::query`] method.
//!
//! ```rust,no_run
//! use dendanskeordbog::{Client, Error};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Error> {
//!     // Create a new client with default settings.
//!     let client = Client::new();
//!
//!     // Query the dictionary for the word "hest".
//!     let document = client.query("hest").await?;
//!
//!     // A document can contain multiple entries (e.g., for homonyms).
//!     if let Some(entry) = document.entries.first() {
//!         println!("Found entry for: {}", entry.head.keyword);
//!         println!("Part of speech: {}", entry.pos);
//!
//!         if let Some(definition) = entry.definitions.first() {
//!             println!("First definition: {}", definition.description);
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```

// Allow repetition of structure name instead of replacing with self as the output from
// rust-analyzer becomes more readable
#![allow(clippy::use_self)]

/// Contains the asynchronous client for querying the dictionary service.
#[cfg(feature = "client")]
pub mod client;

/// Contains the crate's error types.
mod error;

/// Contains all structured data types representing dictionary content.
pub mod types;

/// Re-export of the asynchronous `Client` for convenient access.
#[cfg(feature = "client")]
pub use client::Client;
/// Re-export of the primary `Error` enum for convenient access.
pub use error::Error;
use scraper::{ElementRef, Selector};
/// Re-export of the top-level `DictionaryDocument` struct.
pub use types::DictionaryDocument;

/// A collection of pre-compiled CSS selectors for efficient HTML parsing.
///
/// Use [`Selectors::new`] to construct it.
pub struct Selectors {
    /// Selects the definition level (e.g., "1.a").
    pub level: Selector,
    /// Selects the definition description text.
    pub description: Selector,
    /// Selects example sentences.
    pub example: Selector,
    /// Selects the main article/entry container.
    pub article: Selector,
    /// Selects the head section containing the keyword and audio.
    pub head: Selector,
    /// Selects the part-of-speech information.
    pub pos: Selector,
    /// Selects the morphology/inflection information.
    pub morphology: Selector,
    /// Selects the phonetic transcription.
    pub phonetic: Selector,
    /// Selects nested definition blocks.
    pub definition: Selector,
    /// Selects idiom blocks.
    pub idiom: Selector,
    /// Selects the etymology information.
    pub etymology: Selector,
    /// Selects the main keyword of an entry or idiom.
    pub keyword: Selector,
    /// Selects the `<audio>` element for pronunciation.
    pub audio: Selector,
    /// Selects the phrase text within an idiom.
    pub phrase: Selector,
}

/// A trait for types that can be parsed from an HTML element.
///
/// This provides a common interface for deserializing different parts of a dictionary entry from a
/// `scraper::ElementRef`.
pub trait FromHtml: Sized {
    /// Parses an instance of `Self` from a given HTML element and selectors.
    ///
    /// # Arguments
    ///
    /// * `element` - A reference to the `scraper::ElementRef` to parse from.
    /// * `selectors` - A reference to the pre-compiled `Selectors` struct.
    ///
    /// # Errors
    ///
    /// Returns an `Error` if a required part of the HTML structure is missing or malformed.
    fn from_html_with_selectors(
        element: &ElementRef<'_>,
        selectors: &Selectors,
    ) -> Result<Self, Error>;
}

impl Selectors {
    /// Constructs a new collection of pre-compiled CSS selectors.
    ///
    /// This method initializes all selectors needed to parse the dictionary HTML.
    ///
    /// # Panics
    ///
    /// Panics if any of the hardcoded selector strings are invalid. This indicates a compile-time
    /// programming error, as the selectors are fixed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            level: Selector::parse(":scope > span.l").expect("level selector"),
            description: Selector::parse(":scope > span.dtrn").expect("description selector"),
            example: Selector::parse(":scope > span.ex").expect("example selector"),
            article: Selector::parse("body > span.ar").expect("article selector"),
            head: Selector::parse(":scope > .head").expect("head selector"),
            pos: Selector::parse(":scope > .pos").expect("pos selector"),
            morphology: Selector::parse(":scope > .m").expect("morphology selector"),
            phonetic: Selector::parse(":scope > .phon").expect("phonetic selector"),
            definition: Selector::parse(":scope > span.def > span.def")
                .expect("definition selector"),
            idiom: Selector::parse(":scope > .idiom > .idiom").expect("idiom selector"),
            etymology: Selector::parse(":scope > span.def > span.etym")
                .expect("etymology selector"),
            keyword: Selector::parse(":scope > span.k").expect("keyword span selector"),
            audio: Selector::parse(":scope > span.audio audio").expect("audio span selector"),
            phrase: Selector::parse(":scope > span.k").expect("phrase selector"),
        }
    }
}

impl Default for Selectors {
    /// Creates a default `Selectors` instance by calling [`Selectors::new`].
    fn default() -> Self {
        Self::new()
    }
}
