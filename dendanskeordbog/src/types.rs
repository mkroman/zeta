//! Structured types for representing scraped data from Den Danske Ordbog.
//!
//! This module provides the data structures and parsers used to extract dictionary entries
//! from HTML sources. The main entry point is `DictionaryDocument` which contains all parsed data.
use scraper::{ElementRef, Html, Selector};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "log")]
use tracing::warn;

use crate::{Error, FromHtml, Selectors};

/// Represents a complete dictionary entry from Den Danske Ordbog (The Danish Dictionary).
///
/// This is the main struct that contains all information about a single word entry,
/// including its definitions, pronunciation, etymology, and related terms.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DictionaryEntry {
    /// Unique identifier for this dictionary entry (e.g., "11006626")
    pub id: String,
    /// Header information containing the main word, variants, and pronunciation audio
    pub head: Head,
    /// Part of speech and grammatical information
    pub pos: String,
    /// Morphological information showing word inflections
    /// Examples: "-ken, -ke, -kene" (showing singular/plural forms)
    pub morphology: Option<String>,
    /// Phonetic transcription in IPA notation
    /// Examples: "\[ˈbɔg\]"
    pub phonetic: Option<String>,
    /// All definitions for this word, including nested sub-definitions
    pub definitions: Vec<Definition>,
    /// Etymology information (word origin and history)
    pub etymology: Option<String>,
    /// Idiomatic expressions and phrases containing this word
    pub idioms: Vec<Idiom>,
}

/// Header section of a dictionary entry containing the main word and pronunciation
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Head {
    /// The main dictionary word being defined
    /// Examples: `buk`, `løbe`, `hus`
    pub keyword: String,
    /// Audio pronunciation information if available
    pub audio: Option<Audio>,
}

/// Audio pronunciation data for a dictionary entry
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Audio {
    /// Unique identifier for the audio element (e.g., "`11006626_1`")
    pub id: String,
    /// URL to the pronunciation audio file
    /// Example: `http://static.ordnet.dk/mp3/11006/11006626_1.mp3`
    pub src: String,
}

/// A word definition with potential sub-definitions and related information
///
/// This struct represents the hierarchical nature of dictionary definitions,
/// where a main definition can have numbered sub-definitions (1.a, 1.b, etc.)
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Definition {
    /// The hierarchical level of this definition
    /// Examples: "1", "1.a", "1.b", "2", "2.a"
    pub level: String,
    /// The actual definition text explaining the word's meaning
    /// Examples: "han hos visse drøvtyggere" (male of certain ruminants)
    pub description: String,
    /// Nested sub-definitions under this definition
    pub subdefinitions: Vec<Definition>,
    /// Example sentences or phrases demonstrating usage
    pub examples: Vec<String>,
}

/// An idiomatic expression or phrase containing the defined word
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Idiom {
    /// Unique identifier for this idiom (e.g., "`59002923`")
    pub id: String,
    /// The idiomatic phrase or expression
    /// Examples: "skille fårene fra bukkene" (separate the sheep from the goats)
    pub phrase: String,
    // Definition and examples for this idiomatic expression
    // pub definition: IdiomaticDefinition,
}

/// Definition structure specifically for idiomatic expressions
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct IdiomaticDefinition {
    /// Explanation of what the idiom means
    /// Examples: "sortere de(t) gode fra de(t) dårlige" (sort the good from the bad)
    pub description: String,
    /// Example sentences showing the idiom in use
    pub examples: Vec<String>,
}

/// A complete dictionary document containing one or more entries.
///
/// This struct represents the entire parsed HTML document from a dictionary query.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DictionaryDocument {
    /// A list of all dictionary entries found on the page.
    pub entries: Vec<DictionaryEntry>,
}

impl Audio {
    /// Parses an `Audio` struct from an `<audio>` HTML element.
    fn from_html(elem: &ElementRef<'_>) -> Result<Audio, Error> {
        let id = extract_required_attribute(elem, "id", "audio id")?;
        let src = extract_required_attribute(elem, "src", "audio src")?;

        Ok(Audio { id, src })
    }
}

impl FromHtml for Definition {
    /// Parses a `Definition` from a `<span class="def">` element.
    fn from_html_with_selectors(
        element: &ElementRef<'_>,
        selectors: &Selectors,
    ) -> Result<Self, Error> {
        let level = extract_required_text(element, &selectors.level, "level")?;
        let description = extract_required_text(element, &selectors.description, "description")?;
        // TODO: let subdefinitions: Vec<Definition> = element.select(&definitions_selector);
        let subdefinitions = vec![];
        let examples: Vec<String> = element
            .select(&selectors.example)
            .map(extract_element_text)
            .collect();

        Ok(Definition {
            level,
            description,
            subdefinitions,
            examples,
        })
    }
}

impl DictionaryDocument {
    /// Parses a `DictionaryDocument` from an HTML string.
    ///
    /// This is the main entry point for parsing a complete HTML response body into a structured
    /// `DictionaryDocument`.
    ///
    /// # Arguments
    ///
    /// * `html` - A string slice or `String` containing the full HTML document.
    ///
    /// # Errors
    ///
    /// Returns an `Error` if parsing fails, though the current implementation gracefully handles
    /// missing entries by returning an empty list.
    pub fn from_html<S: AsRef<str>>(html: S) -> Result<Self, Error> {
        let html = html.as_ref();
        let selectors = Selectors::default();
        let document = Html::parse_document(html);
        let entries = Self::parse_entries(&document, &selectors)?;

        Ok(Self { entries })
    }

    /// Parse all dictionary entries from the document.
    ///
    /// This method collects parsing errors for individual entries but continues processing,
    /// allowing partial success when some entries fail to parse.
    fn parse_entries(
        document: &Html,
        selectors: &Selectors,
    ) -> Result<Vec<DictionaryEntry>, Error> {
        let mut entries = Vec::new();
        let mut parse_errors = Vec::new();

        for element in document.select(&selectors.article) {
            match DictionaryEntry::from_html_with_selectors(&element, selectors) {
                Ok(entry) => entries.push(entry),
                Err(err) => {
                    // Log the error but continue processing other entries
                    #[cfg(feature = "log")]
                    warn!(?err, "failed to parse dictionary entry");

                    parse_errors.push(err);
                }
            }
        }

        // If no entries were parsed successfully and we have errors, return the first error
        if entries.is_empty() && !parse_errors.is_empty() {
            return Err(parse_errors.into_iter().next().unwrap());
        }

        Ok(entries)
    }
}

impl FromHtml for DictionaryEntry {
    /// Parses a `DictionaryEntry` from a `<span class="ar">` element.
    fn from_html_with_selectors(
        element: &ElementRef<'_>,
        selectors: &Selectors,
    ) -> Result<Self, Error> {
        let id = extract_required_attribute(element, "id", "entry id")?;
        let head = element
            .select(&selectors.head)
            .next()
            .ok_or_else(|| Error::MissingElement("head".to_string()))
            .and_then(|elem| Head::from_html_with_selectors(&elem, selectors))?;
        let pos = extract_required_text(element, &selectors.pos, "pos")?;
        let morphology = extract_optional_text(element, &selectors.morphology);
        let phonetic =
            extract_optional_text(element, &selectors.phonetic).map(|x| x.trim().to_owned());
        let etymology = extract_optional_text(element, &selectors.etymology);
        let definitions: Vec<Definition> = element
            .select(&selectors.definition)
            .filter_map(|ref elem| Definition::from_html_with_selectors(elem, selectors).ok())
            .collect();
        let idioms: Vec<Idiom> = element
            .select(&selectors.idiom)
            .filter_map(|ref elem| Idiom::from_html_with_selectors(elem, selectors).ok())
            .collect();

        Ok(DictionaryEntry {
            id,
            head,
            pos,
            morphology,
            phonetic,
            definitions,
            etymology,
            idioms,
        })
    }
}

impl Head {
    /// Parses a `Head` struct from a `<span class="head">` HTML element.
    fn from_html_with_selectors(
        elem: &ElementRef<'_>,
        selectors: &Selectors,
    ) -> Result<Head, Error> {
        let keyword = extract_required_text(elem, &selectors.keyword, "keyword")?;
        let audio = elem
            .select(&selectors.audio)
            .next()
            .and_then(|elem| Audio::from_html(&elem).ok());

        Ok(Head { keyword, audio })
    }
}

impl FromHtml for Idiom {
    /// Parses an `Idiom` from a `<span class="idiom">` element.
    fn from_html_with_selectors(
        element: &ElementRef<'_>,
        selectors: &Selectors,
    ) -> Result<Self, Error> {
        let id = extract_required_attribute(element, "id", "idiom id")?;
        let phrase = extract_required_text(element, &selectors.phrase, "phrase")?;

        Ok(Idiom { id, phrase })
    }
}

/// Extracts and concatenates all text nodes from an element into a single string.
fn extract_element_text(elem: ElementRef<'_>) -> String {
    elem.text().collect()
}

/// Extracts a required attribute from an element.
///
/// # Errors
///
/// Returns `Error::MissingElement` with the given `context` if the attribute is not found.
fn extract_required_attribute(
    elem: &ElementRef<'_>,
    attr: &str,
    context: &str,
) -> Result<String, Error> {
    elem.attr(attr)
        .map(ToString::to_string)
        .ok_or_else(|| Error::MissingElement(context.to_string()))
}

/// Finds the first matching child element and extracts its text content.
///
/// # Errors
///
/// Returns `Error::MissingElement` if no element matches the selector.
fn extract_required_text(
    elem: &ElementRef<'_>,
    selector: &Selector,
    context: &str,
) -> Result<String, Error> {
    extract_required_element(elem, selector, context).map(extract_element_text)
}

/// Finds and returns the first required child element matching a selector.
///
/// # Errors
///
/// Returns `Error::MissingElement` if no element matches the selector.
fn extract_required_element<'a>(
    elem: &'a ElementRef<'_>,
    selector: &Selector,
    context: &str,
) -> Result<ElementRef<'a>, Error> {
    elem.select(selector)
        .next()
        .ok_or_else(|| Error::MissingElement(context.to_string()))
}

/// Finds the first matching child element and extracts its text content, if it exists.
fn extract_optional_text(elem: &ElementRef<'_>, selector: &Selector) -> Option<String> {
    elem.select(selector).next().map(extract_element_text)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::*;

    #[test]
    fn test_parse_document() {
        let html = include_str!("../tests/fixtures/queries/hest.html");
        let document = DictionaryDocument::from_html(html).expect("dictionary document");
        let entries = &document.entries;

        assert_eq!(entries.len(), 1);

        let entry = entries.first().unwrap();
        assert_eq!(&entry.id, "11020619");
        assert_eq!(&entry.head.keyword, "hest");
        assert_eq!(
            entry.head.audio.as_ref().map(|audio| audio.id.as_str()),
            Some("11020619_1")
        );
        assert_eq!(
            entry.head.audio.as_ref().map(|audio| audio.src.as_str()),
            Some("http://static.ordnet.dk/mp3/11020/11020619_1.mp3")
        );
        assert_eq!(&entry.pos, "substantiv, fælleskøn");
        assert_eq!(entry.morphology, Some("-en, -e, -ene".to_string()));
        assert_eq!(entry.phonetic, Some("[ˈhεsd]".to_string()));

        assert_eq!(entry.definitions.len(), 4);

        let definition = entry.definitions.first().unwrap();
        assert_eq!(definition.description, "hovdyr med stort, aflangt hoved, lange, slanke ben, manke og en hale med lange hår • anvendes især som ride- og trækdyr • tilhører hestefamilien, der også omfatter zebraer, æsler og vildheste".to_string());
        assert_eq!(definition.examples.len(), 2);
        let example = definition.examples.last().unwrap();
        assert_eq!(
            example,
            "Rytterne standsede, da den forreste af dem hævede hånden. Hans hest stejlede og vrinskede"
        );
        assert_eq!(entry.etymology, Some("norrønt hestr; med oprindelig betydning 'den bedst springende' og oprindelig kun om 'hingst', muligvis erstatningsord for det mere udbredte indoeuropæiske ord for dyret, fx i latin equus".to_string()));
    }

    fn query_fixture_paths() -> Vec<PathBuf> {
        fs::read_dir("tests/fixtures/queries")
            .expect("could not read queries fixtures directory")
            .filter_map(|entry| entry.map(|inner| inner.path()).ok())
            .collect()
    }

    #[test]
    fn test_parse_query_fixtures() {
        for path in query_fixture_paths() {
            let html = fs::read_to_string(path).unwrap();
            let document = DictionaryDocument::from_html(&html).expect("no entries");

            assert!(!document.entries.is_empty());
        }
    }
}
