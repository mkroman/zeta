//! Structured types
use scraper::{ElementRef, Html, Selector};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::Error;

/// Represents a complete dictionary entry from Den Danske Ordbog (The Danish Dictionary)
///
/// This is the main struct that contains all information about a single word entry,
/// including its definitions, pronunciation, etymology, and related terms.
///
/// # HTML Source
/// Parsed from `<span class="ar" id="...">` elements
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DictionaryEntry {
    /// Unique identifier for this dictionary entry (e.g., "11006626")
    /// Found in the `id` attribute of the root `<span class="ar">` element
    pub id: String,

    /// Header information containing the main word, variants, and pronunciation audio
    /// Parsed from `<span class="head">` element
    pub head: Head,

    /// Part of speech and grammatical information
    /// Examples: "substantiv, fælleskøn" (noun, common gender)
    /// Parsed from `<span class="pos">` element
    pub pos: String,

    /// Morphological information showing word inflections
    /// Examples: "-ken, -ke, -kene" (showing singular/plural forms)
    /// Parsed from `<span class="m">` element, may be absent for some entries
    pub morphology: Option<String>,

    /// Phonetic transcription in IPA notation
    /// Examples: "\[ˈbɔg\]"
    /// Parsed from `<span class="phon">` element
    pub phonetic: Option<String>,

    /// All definitions for this word, including nested sub-definitions
    /// Parsed from `<span class="def">` elements with hierarchical structure
    pub definitions: Vec<Definition>,

    /// Idiomatic expressions and phrases containing this word
    /// Parsed from `<span class="idiom">` elements
    pub idioms: Vec<Idiom>,
}

/// Header section of a dictionary entry containing the main word and pronunciation
///
/// # HTML Source
/// Parsed from `<span class="head">` element
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Head {
    /// The main dictionary word being defined
    ///
    /// Examples: `buk`, `løbe`, `hus`
    ///
    /// Parsed from `<span class="k">` element within the head
    pub keyword: String,
    /// Audio pronunciation information if available
    /// Contains both the audio file URL and player controls
    pub audio: Option<Audio>,
}

/// Audio pronunciation data for a dictionary entry
///
/// # HTML Source
/// Parsed from `<span class="audio">` containing `<audio>` and link elements
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Audio {
    /// Unique identifier for the audio element (e.g., "`11006626_1`")
    ///
    /// Found in the `id` attribute of the `<audio>` element
    pub id: String,

    /// URL to the MP3 pronunciation file
    ///
    /// Examples: `http://static.ordnet.dk/mp3/11006/11006626_1.mp3`
    ///
    /// Found in the `src` attribute of the `<audio>` element
    pub src: String,
}

/// A word definition with potential sub-definitions and related information
///
/// This struct represents the hierarchical nature of dictionary definitions,
/// where a main definition can have numbered sub-definitions (1.a, 1.b, etc.)
///
/// # HTML Source
/// Parsed from `<span class="def">` elements, which can be nested
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Definition {
    /// The hierarchical level of this definition
    /// Examples: "1", "1.a", "1.b", "2", "2.a"
    /// Parsed from `<span class="l">` element, may be empty for root definitions
    pub level: String,

    /// The actual definition text explaining the word's meaning
    /// Examples: "han hos visse drøvtyggere" (male of certain ruminants)
    /// Parsed from `<span class="dtrn">` element
    pub description: String,

    /// Etymology information (word origin and history)
    /// Usually only present on the top-level definition
    /// Parsed from `<span class="etym">` element
    pub etymology: Option<String>,

    /// Nested sub-definitions under this definition
    /// Represents the hierarchical structure like 1 → 1.a → 1.b
    pub subdefinitions: Vec<Definition>,

    /// Example sentences or phrases demonstrating usage
    /// Parsed from `<span class="ex">` elements
    pub examples: Vec<String>,
}

/// An idiomatic expression or phrase containing the defined word
///
/// # HTML Source
/// Parsed from `<span class="idiom">` elements with their own IDs
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Idiom {
    /// Unique identifier for this idiom (e.g., "59002923")
    /// Found in the `id` attribute of the idiom span
    pub id: String,

    /// The idiomatic phrase or expression
    /// Examples: "skille fårene fra bukkene" (separate the sheep from the goats)
    /// Parsed from the first `<span class="k">` element within the idiom
    pub phrase: String,

    /// Definition and examples for this idiomatic expression
    pub definition: String, // IdiomaticDefinition,
}

/// Definition structure specifically for idiomatic expressions
///
/// Similar to regular definitions but tailored for phrases and idioms
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct IdiomaticDefinition {
    /// Explanation of what the idiom means
    /// Examples: "sortere de(t) gode fra de(t) dårlige" (sort the good from the bad)
    /// Parsed from `<span class="dtrn">` within the idiom definition
    pub description: String,
    /// Example sentences showing the idiom in use
    /// Parsed from `<span class="ex">` elements within the idiom
    pub examples: Vec<String>,
}

/// Complete dictionary document structure including metadata
///
/// This represents the entire HTML document, not just the dictionary entry
///
/// # HTML Source
/// Parsed from the complete HTML document structure
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DictionaryDocument {
    /// The main dictionary entry content
    pub entries: Vec<DictionaryEntry>,
}

impl Audio {
    fn from_html(elem: &ElementRef) -> Result<Audio, Error> {
        let id = elem
            .attr("id")
            .ok_or_else(|| Error::MissingElement("audio id"))?
            .to_string();
        let src = elem
            .attr("src")
            .ok_or_else(|| Error::MissingElement("audio src"))?
            .to_string();

        Ok(Audio { id, src })
    }
}

impl Definition {
    pub fn from_html(element: &scraper::ElementRef) -> Result<Self, Error> {
        let level_selector = Selector::parse(":scope > span.l").expect("level selector");
        let description_selector =
            Selector::parse(":scope > span.dtrn").expect("description selector");
        let etymology_selector = Selector::parse(":scope > span.etym").expect("etymology selector");
        let example_selector = Selector::parse(":scope > span.ex").expect("example selector");

        let level = element
            .select(&level_selector)
            .next()
            .map(element_text)
            .ok_or_else(|| Error::MissingElement("span.l"))?;
        let description = element
            .select(&description_selector)
            .next()
            .map(element_text)
            .ok_or_else(|| Error::MissingElement("description"))?;
        let etymology = element.select(&etymology_selector).next().map(element_text);
        // TODO: let subdefinitions: Vec<Definition> = element.select(&definitions_selector);
        let subdefinitions = vec![];
        let examples: Vec<String> = element
            .select(&example_selector)
            .map(element_text)
            .collect();

        Ok(Definition {
            level,
            description,
            etymology,
            subdefinitions,
            examples,
        })
    }
}

impl Head {
    fn from_html(elem: &ElementRef) -> Result<Head, Error> {
        let keyword_selector = Selector::parse(":scope > span.k").expect("keyword span selector");
        let audio_selector =
            Selector::parse(":scope > span.audio audio").expect("audio span selector");

        let keyword = elem
            .select(&keyword_selector)
            .next()
            .map(element_text)
            .ok_or_else(|| Error::MissingElement("keyword"))?;

        let audio = elem
            .select(&audio_selector)
            .next()
            .map(|ref elem| Audio::from_html(elem).ok())
            .ok_or_else(|| Error::MissingElement("keyword"))?;

        Ok(Head { keyword, audio })
    }
}

impl Idiom {
    pub fn from_html(element: &scraper::ElementRef) -> Result<Self, Error> {
        let phrase_selector = Selector::parse(":scope > span.k").expect("phrase selector");

        let id = element
            .attr("id")
            .ok_or_else(|| Error::MissingElement("idiom id"))?
            .to_string();
        let phrase = element.select(&phrase_selector).map(element_text).collect();

        Ok(Idiom {
            id,
            phrase,
            definition: String::new(),
        })
    }
}

impl DictionaryDocument {
    pub fn from_html(html: impl AsRef<str>) -> Result<DictionaryDocument, Error> {
        let html = html.as_ref();
        let article_selector = Selector::parse("body > span.ar").unwrap();

        let document = Html::parse_document(html);
        let entries: Vec<DictionaryEntry> = document
            .select(&article_selector)
            .filter_map(|ref elem| DictionaryEntry::from_html(elem).ok())
            .collect();

        Ok(DictionaryDocument { entries })
    }
}

// Implementation with parsing logic (would require scraper or similar crate)
impl DictionaryEntry {
    /// Parse a dictionary entry from HTML element
    pub fn from_html(element: &scraper::ElementRef) -> Result<Self, Error> {
        let head_selector = Selector::parse(":scope > .head").expect("head selector");
        let pos_selector = Selector::parse(":scope > .pos").expect("pos selector");
        let morphology_selector = Selector::parse(":scope > .m").expect("morphology selector");
        let phonetic_selector = Selector::parse(":scope > .phon").expect("phonetic selector");
        let definition_selector =
            Selector::parse(":scope > span.def > span.def").expect("definition selector");
        let idiom_selector = Selector::parse(":scope > .idiom > .idiom").expect("idiom selector");

        let id = element
            .attr("id")
            .ok_or_else(|| Error::MissingElement("idiom id"))?
            .to_string();
        let head = element
            .select(&head_selector)
            .next()
            .map(|ref elem| Head::from_html(elem))
            .ok_or_else(|| Error::MissingElement("head"))??;
        let pos = element
            .select(&pos_selector)
            .next()
            .map(element_text)
            .ok_or_else(|| Error::MissingElement("pos span"))?;
        let morphology = element
            .select(&morphology_selector)
            .next()
            .map(element_text);
        let phonetic = element
            .select(&phonetic_selector)
            .next()
            .map(|elem| elem.text().map(str::trim).collect());
        let definitions: Vec<Definition> = element
            .select(&definition_selector)
            .filter_map(|ref elem| Definition::from_html(elem).ok())
            .collect();
        let idioms: Vec<Idiom> = element
            .select(&idiom_selector)
            .filter_map(|ref elem| Idiom::from_html(elem).ok())
            .collect();

        Ok(DictionaryEntry {
            id,
            head,
            pos,
            morphology,
            phonetic,
            definitions,
            idioms,
        })
    }
}

fn element_text(elem: ElementRef) -> String {
    elem.text().collect()
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
