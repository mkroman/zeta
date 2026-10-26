//! The parsing primitives shared by the dictionary types.

use std::sync::LazyLock;

use scraper::{ElementRef, Selector};

use crate::Error;

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

/// The selectors shared by every parse: the selector strings are fixed, so compiling them once
/// avoids rebuilding the same 14 selectors for each response.
static SHARED_SELECTORS: LazyLock<Selectors> = LazyLock::new(Selectors::new);

impl Selectors {
    /// Returns the process-wide compiled selector set.
    #[must_use]
    pub fn shared() -> &'static Self {
        &SHARED_SELECTORS
    }
}
