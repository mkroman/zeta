//! Unicode character names, general categories and emoji sequence names.
//!
//! The image format is called Nomina (Latin for names). The `zeta-unicode-build` crate trains
//! one flat Nomina image from the vendored UCD files and links it into the binary with
//! `include_bytes!`. A lookup decodes at most one front-coded block, so there is no full name
//! arena; `write_name` does not allocate, while `name` returns an owned [`String`]. Hangul
//! syllables and `*`-pattern names are generated from their code point.
//! Emoji keys are canonicalized (`U+FE0F` is stripped) and skin-tone variants are synthesized
//! from a 3-bit rule plus a 2-bit published-tone-count mask, so 1,944 stored keys answer for
//! every one of the 3,972 sequences the UCD names.
//!
//! ```
//! use zeta_unicode::{emoji_name, general_category, name, name_or_label};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let latin = name('ß').ok_or("missing name")?;
//!     assert_eq!(latin.as_str(), "LATIN SMALL LETTER SHARP S");
//!     assert_eq!(general_category('ß').long_name(), "Lowercase Letter");
//!     let rainbow = emoji_name("🏳️‍🌈").ok_or("missing emoji name")?;
//!     assert_eq!(rainbow.as_ref(), "rainbow flag");
//!     assert_eq!(name_or_label('\u{f8ff}'), "<private-use-F8FF>");
//!     Ok(())
//! }
//! ```
//!
//! # Panics
//!
//! The first name lookup on a thread panics if the generated image is malformed; the first
//! emoji or general-category lookup panics process-wide for the same reason. The build verifies
//! every section, so this is a build invariant rather than a runtime condition.

use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::OnceLock;

use zeta_unicode_format::{MAX_KEY, UNICODE_END};

use nomina::Nomina;
use nomina::emoji::Emoji;
use nomina::gc::Gc;
use nomina::image;

#[doc(inline)]
pub use gc::GeneralCategory;

mod gc;
mod nomina;

include!(concat!(env!("OUT_DIR"), "/zeta_unicode_version.rs"));

/// The target size budget for the Nomina image, in bytes.
///
/// This is the measured random-access budget, not the smaller full-decode budget: the Nomina
/// image trades table bytes for a tiny decoder and a sub-100 KB heap.
pub const TABLE_BUDGET: usize = 200_000;

/// The longest emoji key the table stores, in bytes.
///
/// `.uni` uses it to bound how far it probes for a multi-character emoji sequence.
pub const MAX_EMOJI_KEY_BYTES: usize = MAX_KEY;

/// The generated Nomina image, linked in at compile time.
static IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/zeta_unicode_nomina.bin"));

/// The parsed emoji section, initialized on the first emoji lookup.
static EMOJI: OnceLock<Emoji> = OnceLock::new();
/// The parsed general-category section, initialized on the first category lookup.
static GC: OnceLock<Gc> = OnceLock::new();

thread_local! {
    /// The per-thread name decoder. Its fixed scratch buffer is the only mutable state left
    /// after initialization, so lookups do not allocate or share a lock.
    static NAMES: RefCell<Option<Nomina>> = const { RefCell::new(None) };
}

/// Runs `f` with this thread's name decoder, building it on first use.
fn with_nomina<R>(f: impl FnOnce(&mut Nomina) -> R) -> R {
    NAMES.with(|cell| {
        let mut slot = cell.borrow_mut();
        let nomina = slot.get_or_insert_with(|| {
            Nomina::new(IMAGE).expect("the generated Nomina image is valid")
        });
        f(nomina)
    })
}

/// Returns the parsed emoji section, building it on first use.
fn emoji() -> &'static Emoji {
    EMOJI.get_or_init(|| {
        let section =
            image::section(IMAGE, image::EMOJI).expect("the Nomina image has an emoji section");
        Emoji::parse(section).expect("the generated emoji section is valid")
    })
}

/// Returns the parsed general-category section, building it on first use.
fn gc() -> &'static Gc {
    GC.get_or_init(|| {
        let section = image::section(IMAGE, image::GC)
            .expect("the Nomina image has a general-category section");
        Gc::parse(section).expect("the generated general-category section is valid")
    })
}

/// Warms the process-wide emoji and general-category tables.
///
/// The name decoder is per thread and has no shared state, so it stays lazy: a call on a
/// throwaway thread would warm nothing a handler later uses. A warm call is cheap and
/// idempotent.
pub fn init() {
    emoji();
    gc();
}

/// Returns the `Name` property of `c`, a formal alias from `NameAliases.txt` when the `Name`
/// property is empty, or the derived name Unicode defines algorithmically.
///
/// Stored names are decoded from one block, so they are returned as an owned [`String`]. Use
/// [`write_name`] for the zero-allocation path.
#[must_use]
pub fn name(c: char) -> Option<String> {
    with_nomina(|nomina| nomina.name(u32::from(c)))
}

/// Appends the name of `c` to `buf`, returning `false` when `c` has no name.
///
/// The decoder writes directly into `buf`; a stored name decodes at most one block,
/// and Hangul or pattern names are generated without a temporary allocation.
///
/// # Examples
///
/// ```
/// let mut buf = String::new();
/// assert!(zeta_unicode::write_name('中', &mut buf));
/// assert_eq!(buf, "CJK UNIFIED IDEOGRAPH-4E2D");
/// ```
#[must_use = "the return value reports whether `c` has a name"]
pub fn write_name(c: char, buf: &mut String) -> bool {
    with_nomina(|nomina| nomina.write_name(u32::from(c), buf))
}

/// Returns the name of `c`, or the label Unicode defines for code points without one.
///
/// Labels are `<private-use-F8FF>`, `<reserved-0378>`, `<noncharacter-FFFE>` and
/// `<control-0000>`; controls with a formal alias answer with the alias instead, so the control
/// label is the fallback.
#[must_use]
pub fn name_or_label(c: char) -> String {
    let code_point = u32::from(c);
    let mut buffer = String::new();
    if with_nomina(|nomina| nomina.write_name(code_point, &mut buffer)) {
        return buffer;
    }

    let kind = match general_category(c) {
        GeneralCategory::PrivateUse => "private-use",
        GeneralCategory::Control => "control",
        GeneralCategory::Surrogate => "surrogate",
        GeneralCategory::Unassigned if is_noncharacter(code_point) => "noncharacter",
        _ => "reserved",
    };
    format!("<{kind}-{code_point:04X}>")
}

/// Returns the general category of `c`; unassigned points answer with
/// [`GeneralCategory::Unassigned`].
#[must_use]
pub fn general_category(c: char) -> GeneralCategory {
    gc().general_category(u32::from(c))
        .and_then(|id| GeneralCategory::from_id(usize::from(id)))
        .unwrap_or(GeneralCategory::Unassigned)
}

/// Is `cp` one of Unicode's noncharacters (`U+FDD0..U+FDEF` or `*FFFE`/`*FFFF`)?
///
/// Values above `U+10FFFF` are not code points and answer `false`.
#[must_use]
pub const fn is_noncharacter(cp: u32) -> bool {
    cp <= UNICODE_END && (matches!(cp, 0xFDD0..=0xFDEF) || matches!(cp & 0xFFFF, 0xFFFE | 0xFFFF))
}

/// Returns the CLDR short name of an emoji sequence such as `🏳️‍🌈`.
///
/// Keys are canonicalized by dropping `U+FE0F`; skin-tone variants resolve by stripping the
/// tones and replaying the base's formatting rule. Returns `None` for non-emoji text and for
/// sequences whose base or published tone count is unknown.
#[must_use]
pub fn emoji_name(sequence: &str) -> Option<Cow<'static, str>> {
    let table = emoji();
    with_nomina(|nomina| {
        table.emoji_name(
            &mut |cp: u32, out: &mut String| nomina.write_name(cp, out),
            sequence,
        )
    })
}

/// Returns whether `sequence` could start a stored emoji sequence.
///
/// This is an admission check, not a match: the first byte is tested against a bitmap built
/// from every stored key. Empty input answers `false`.
#[must_use]
pub fn may_start_emoji_sequence(sequence: &str) -> bool {
    sequence
        .as_bytes()
        .first()
        .is_some_and(|&byte| emoji().may_start(byte))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use bytes::Buf;

    use super::*;

    /// The UCD directory under test: `ZETA_UNICODE_DATA_DIR` when set, otherwise the vendored
    /// `data/` directory. This lets the same tests run against an older or newer release.
    fn data_dir() -> std::path::PathBuf {
        std::env::var_os("ZETA_UNICODE_DATA_DIR").map_or_else(
            || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data"),
            std::path::PathBuf::from,
        )
    }

    /// Reads one UCD text from a flat or versioned layout.
    fn read_ucd(name: &str) -> String {
        let dir = data_dir();
        let mut candidates = vec![dir.join(name)];
        for prefix in ["extracted", "ucd", "ucd/extracted"] {
            candidates.push(dir.join(prefix).join(name));
        }
        if name == "emoji-test.txt" {
            candidates.push(dir.join("emoji/emoji-test.txt"));
            candidates.push(dir.join("ucd/emoji/emoji-test.txt"));
        }
        let path = candidates
            .into_iter()
            .find(|path| path.is_file())
            .unwrap_or_else(|| panic!("no UCD file {name} under {}", dir.display()));
        std::fs::read_to_string(path).expect("read the UCD file")
    }

    /// The data records of a UCD file, without its blank and comment lines.
    fn records(text: &str) -> impl Iterator<Item = &str> {
        text.lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
    }

    /// Parses one hexadecimal code point.
    fn code_point(text: &str) -> u32 {
        u32::from_str_radix(text.trim(), 16).expect("a hexadecimal code point")
    }

    /// Is the Unicode release under test at least `(major, minor)`?
    ///
    /// The sample assertions below name emoji that specific releases introduced; gating them
    /// on the generated [`UNICODE_VERSION`] keeps the suite meaningful on the vendored data
    /// and passing against an older `ZETA_UNICODE_DATA_DIR`.
    fn release_at_least(major: u8, minor: u8) -> bool {
        (major, minor, 0) <= UNICODE_VERSION
    }

    /// Parses a `start..end` range, or a single code point as a one-element range.
    fn range(text: &str) -> (u32, u32) {
        let text = text.trim();
        if let Some((start, end)) = text.split_once("..") {
            (code_point(start), code_point(end))
        } else {
            let cp = code_point(text);
            (cp, cp)
        }
    }

    /// Reads the version from the Nomina image header.
    fn image_version() -> (u8, u8, u8) {
        let mut image = IMAGE;
        image.advance(4);
        let major = image.try_get_u16_le().expect("major version");
        let minor = image.try_get_u16_le().expect("minor version");
        let patch = image.try_get_u16_le().expect("patch version");
        (
            u8::try_from(major).expect("major version"),
            u8::try_from(minor).expect("minor version"),
            u8::try_from(patch).expect("patch version"),
        )
    }

    #[test]
    fn generated_version_matches_the_table_header() {
        assert_eq!(
            image_version(),
            UNICODE_VERSION,
            "the generated image was built from a different Unicode release"
        );
    }

    #[test]
    fn every_name_in_the_ucd_round_trips() {
        let mut checked = 0usize;

        let derived = read_ucd("DerivedName.txt");
        for line in records(&derived) {
            let (lhs, rhs) = line.split_once(';').expect("field separator");
            let lhs = lhs.trim();

            // `DerivedName.txt` spells a pattern as a `*` placeholder "in place of the code
            // point in hex", on a single-code-point line as well as on a range: U+18CFF is the
            // one that matters, since it is the only point whose line carries a `*`.
            let value = rhs.trim();
            if let Some((prefix, _width)) = value.split_once('*') {
                let (start, end) = range(lhs);
                for cp in start..=end {
                    let Some(c) = char::from_u32(cp) else {
                        continue;
                    };
                    assert_eq!(
                        name(c).as_deref(),
                        Some(format!("{prefix}{cp:04X}").as_str()),
                        "pattern name for U+{cp:04X}"
                    );
                    checked += 1;
                }
                continue;
            }

            let cp = code_point(lhs);
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            assert_eq!(name(c).as_deref(), Some(rhs.trim()), "name for U+{cp:04X}");
            checked += 1;
        }

        assert!(checked > 100_000, "only checked {checked} names");
    }

    #[test]
    fn alias_backed_names_are_available() {
        let mut checked = 0usize;

        let aliases = read_ucd("NameAliases.txt");
        for line in records(&aliases) {
            let cp = code_point(line.split(';').next().expect("code point"));
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            assert!(
                name(c).is_some(),
                "U+{cp:04X} has an alias but answered with no name"
            );
            checked += 1;
        }

        assert!(checked > 300, "only checked {checked} aliases");
        assert_eq!(name('\u{16}').as_deref(), Some("SYNCHRONOUS IDLE"));
        assert_eq!(name('\u{9}').as_deref(), Some("CHARACTER TABULATION"));
    }

    #[test]
    fn general_categories_match_the_ucd() {
        let mut checked = 0usize;

        let derived_gc = read_ucd("DerivedGeneralCategory.txt");
        for line in records(&derived_gc) {
            let (lhs, rest) = line.split_once(';').expect("field separator");
            let letter = rest.split('#').next().expect("category").trim();
            let (start, end) = range(lhs);

            for cp in start..=end {
                let Some(c) = char::from_u32(cp) else {
                    continue;
                };
                assert_eq!(
                    general_category(c).short_name(),
                    letter,
                    "general category of U+{cp:04X}"
                );
                checked += 1;
            }
        }

        // Surrogates are not `char` values; everything else must be covered.
        assert!(checked > 1_100_000, "only checked {checked} code points");
        assert_eq!(general_category('A'), GeneralCategory::UppercaseLetter);
        assert_eq!(general_category('あ'), GeneralCategory::OtherLetter);
        assert_eq!(general_category('\u{378}'), GeneralCategory::Unassigned);
    }

    /// Rebuilds the emoji oracle the way `zeta-unicode-build` does: canonical keys (U+FE0F
    /// removed) with named-sequence names taking precedence over fully-qualified, then
    /// qualified over unqualified. `non-fully-qualified` ranks with `unqualified`, as Emoji
    /// 5.0 and 11.0 use it for sequences that later releases re-qualified.
    fn emoji_oracle() -> BTreeMap<String, String> {
        /// Keeps the higher-precedence name for a canonical key; a tie keeps the later entry.
        fn keep_best(
            entries: &mut BTreeMap<Vec<u32>, (u8, String)>,
            cps: Vec<u32>,
            rank: u8,
            name: String,
        ) {
            match entries.get(&cps) {
                Some((best, _)) if *best > rank => {}
                _ => {
                    entries.insert(cps, (rank, name));
                }
            }
        }

        /// The canonical form of a sequence: `U+FE0F` removed.
        fn canonical(cps: &[u32]) -> Vec<u32> {
            cps.iter().copied().filter(|&cp| cp != 0xFE0F).collect()
        }

        let emoji_test = read_ucd("emoji-test.txt");
        let named_sequences = read_ucd("NamedSequences.txt");
        let mut ranked: BTreeMap<Vec<u32>, (u8, String)> = BTreeMap::new();

        for line in records(&emoji_test) {
            let (left, rest) = line.split_once(';').expect("field separator");
            let (status, comment) = rest.split_once('#').expect("comment");
            let cps: Vec<u32> = left.split_whitespace().map(code_point).collect();
            let rank = match status.trim() {
                "fully-qualified" | "component" => 2,
                "minimally-qualified" => 1,
                "unqualified" | "non-fully-qualified" => 0,
                other => panic!("unknown status {other}"),
            };
            let words: Vec<&str> = comment.split_whitespace().collect();
            assert!(words.len() >= 2, "unexpected emoji comment: {comment}");
            // Modern files spell `# <emoji> E1.0 <name>`; older files omit the `E<version>`
            // word. Handle both so the same test runs against old and new UCD releases.
            let name_at = usize::from(
                words
                    .get(1)
                    .is_some_and(|word| word.starts_with('E') && word.contains('.')),
            ) + 1;
            keep_best(
                &mut ranked,
                canonical(&cps),
                rank,
                words[name_at..].join(" "),
            );
        }

        // `NamedSequences.txt` also carries non-emoji sequences (Latin letters with combining
        // marks); the build drops those, so the oracle must too. The keycap names override the
        // test file's lower-case ones.
        for line in records(&named_sequences) {
            let (name, cps) = line.split_once(';').expect("field separator");
            let cps: Vec<u32> = cps.split_whitespace().map(code_point).collect();
            let canonical = canonical(&cps);
            if !ranked.contains_key(&canonical) {
                continue;
            }
            keep_best(&mut ranked, canonical, 3, name.trim().to_owned());
        }

        let mut oracle = BTreeMap::new();
        for (cps, (_, name)) in ranked {
            let key: String = cps.iter().filter_map(|&cp| char::from_u32(cp)).collect();
            oracle.insert(key, name);
        }
        oracle
    }

    #[test]
    fn emoji_sequences_resolve_to_their_names() {
        let oracle = emoji_oracle();

        for (key, name) in &oracle {
            assert_eq!(
                emoji_name(key).as_deref(),
                Some(name.as_str()),
                "emoji name for {key:?}"
            );
        }

        // The keycap named sequences only supply names for existing keys; every canonical
        // key must resolve. Emoji 5.0 (Unicode 10.0), the oldest supported release, answers
        // for about 2,600 keys, and modern releases for nearly 4,000 (5,244 emoji-test
        // entries collapse onto 3,972 canonical keys once U+FE0F is merged away).
        let minimum = if release_at_least(12, 0) {
            3_000
        } else {
            2_500
        };
        assert!(
            oracle.len() > minimum,
            "only checked {} sequences",
            oracle.len()
        );
        assert_eq!(emoji_name("🏳️‍🌈").as_deref(), Some("rainbow flag"));
        assert_eq!(emoji_name("2️⃣").as_deref(), Some("KEYCAP DIGIT TWO"));
        // Text presentation answers exactly like emoji presentation.
        assert_eq!(emoji_name("🚶‍♂").as_deref(), Some("man walking"));
        // A skin-tone variant is synthesized from its base and the 3-bit rule.
        assert_eq!(
            emoji_name("🖐🏻").as_deref(),
            Some("hand with fingers splayed: light skin tone")
        );
        if release_at_least(14, 0) {
            assert_eq!(
                emoji_name("🫶🏻").as_deref(),
                Some("heart hands: light skin tone")
            );
        }
        assert_eq!(emoji_name("d'être"), None);
        assert_eq!(emoji_name("🐱‍🏍"), None);
        // A non-emoji named sequence (a decomposed Latin letter) is not an emoji.
        assert_eq!(emoji_name("\u{0100}\u{0300}"), None);
    }

    #[test]
    fn tone_fallback_respects_published_tone_counts() {
        // Two tones on a base that only publishes one-tone variants.
        assert_eq!(emoji_name("👍🏻🏿"), None);
        if release_at_least(12, 0) {
            // Published counts still resolve, and same-tone groups deduplicate the phrase.
            assert_eq!(
                emoji_name("👩🏻‍🤝‍👩🏻").as_deref(),
                Some("women holding hands: light skin tone")
            );
            assert_eq!(
                emoji_name("🧑🏻‍🤝‍🧑🏻").as_deref(),
                Some("people holding hands: light skin tone")
            );
        }
        if release_at_least(14, 0) {
            // One tone on a base that only publishes two-tone variants.
            assert_eq!(emoji_name("🫱🏻‍🫲"), None);
            assert_eq!(
                emoji_name("🫱🏻‍🫲🏻").as_deref(),
                Some("handshake: light skin tone")
            );
        }
    }

    #[test]
    fn leading_tone_or_variation_selector_is_not_a_sequence() {
        assert_eq!(emoji_name("🏻👋"), None);
        assert_eq!(emoji_name("\u{FE0F}👍"), None);
        if release_at_least(11, 0) {
            // A lone skin-tone component is still an entry. Emoji data only lists the
            // components from Emoji 11.0 on; older files carry no standalone rows for them.
            assert_eq!(emoji_name("🏻").as_deref(), Some("light skin tone"));
        }
    }

    #[test]
    fn unnamed_code_points_get_a_label() {
        assert_eq!(name_or_label('\u{f8ff}'), "<private-use-F8FF>");
        assert_eq!(name_or_label('\u{378}'), "<reserved-0378>");
        assert_eq!(name_or_label('\u{FFFE}'), "<noncharacter-FFFE>");
        assert_eq!(name('\u{0}').as_deref(), Some("NULL"));
        // Controls answer with their formal alias rather than the `<control-…>` label.
        assert_eq!(name_or_label('\u{0}'), "NULL");
        assert_eq!(name_or_label('\u{80}'), "PADDING CHARACTER");
    }

    #[test]
    fn name_decodes_stored_and_generated_names() {
        // Stored names are decoded from one block, so the Nomina backend returns an owned name.
        assert_eq!(name('A').as_deref(), Some("LATIN CAPITAL LETTER A"));
        assert_eq!(name('中').as_deref(), Some("CJK UNIFIED IDEOGRAPH-4E2D"));
        assert!(emoji_name("🦔").is_some());
        if release_at_least(14, 0) {
            assert!(matches!(emoji_name("🫶🏻"), Some(Cow::Owned(_))));
        }
    }

    #[test]
    fn init_is_idempotent_and_lookups_are_stable() {
        let name_before = name('A');
        let emoji_before = emoji_name("🏳️‍🌈");

        init();
        init();

        assert_eq!(name('A'), name_before);
        assert_eq!(emoji_name("🏳️‍🌈"), emoji_before);
    }

    #[test]
    fn noncharacter_boundaries_are_exact() {
        for cp in [0xFDD0, 0xFDEF, 0xFFFE, 0xFFFF, 0x0001_FFFE, 0x0010_FFFF] {
            assert!(is_noncharacter(cp), "U+{cp:04X}");
        }
        for cp in [0xFDCF, 0xFDF0, 0x0001_FDD0, 0x0011_FFFE, 0x0011_0000] {
            assert!(!is_noncharacter(cp), "U+{cp:04X}");
        }
    }

    #[test]
    fn write_name_matches_name() {
        let mut buf = String::from("x");
        assert!(write_name('中', &mut buf));
        assert_eq!(buf, "xCJK UNIFIED IDEOGRAPH-4E2D");

        buf.clear();
        assert!(write_name('가', &mut buf));
        assert_eq!(buf, "HANGUL SYLLABLE GA");

        buf.clear();
        assert!(!write_name('\u{378}', &mut buf));
        assert_eq!(buf, "");
    }

    #[test]
    fn pattern_names_use_unicode_hex() {
        let derived = read_ucd("DerivedName.txt");
        let (start, want) = records(&derived)
            .find_map(|line| {
                let (lhs, rhs) = line.split_once(';')?;
                let (prefix, suffix) = rhs.trim().split_once('*')?;
                let start = lhs
                    .trim()
                    .split_once("..")
                    .map_or_else(|| lhs.trim(), |(start, _)| start);
                let cp = code_point(start);
                Some((cp, format!("{prefix}{cp:04X}{suffix}")))
            })
            .expect("a pattern range");
        assert_eq!(
            name(char::from_u32(start).expect("a scalar code point")).as_deref(),
            Some(want.as_str())
        );
        assert_eq!(name('中').as_deref(), Some("CJK UNIFIED IDEOGRAPH-4E2D"));
    }

    /// Prints the image size and holds it to the Nomina budget.
    #[test]
    fn table_stays_within_the_nomina_budget() {
        eprintln!("Nomina image: {} bytes", IMAGE.len());
        assert!(
            IMAGE.len() <= TABLE_BUDGET,
            "image grew to {} bytes",
            IMAGE.len()
        );
    }

    /// Times every public lookup over its whole domain, so a layout change can be compared on
    /// both architectures:
    ///
    /// ```sh
    /// cargo test -p zeta-unicode --release -- --ignored --nocapture benchmarks
    /// ```
    #[test]
    #[ignore = "benchmark"]
    fn benchmarks() {
        use std::time::{Duration, Instant};

        /// Named characters per name run, and emoji probes per emoji run.
        const SAMPLES: u32 = 1 << 16;
        /// Every code point in the Unicode range.
        const ALL_CODE_POINTS: u32 = 0x0011_0000;

        fn per_lookup(elapsed: Duration, samples: u32) -> f64 {
            elapsed.as_secs_f64() * 1e9 / f64::from(samples)
        }

        /// Runs `work` four times, printing the last round's per-lookup time; the opening
        /// passes pay the page faults.
        fn measure(label: &str, samples: u32, mut work: impl FnMut() -> u64) {
            for round in 0..4 {
                let start = Instant::now();
                let sink = work();
                if round == 3 {
                    eprintln!(
                        "{label:<17} {:>8.2} ns/lookup (sink {sink})",
                        per_lookup(start.elapsed(), samples)
                    );
                }
            }
        }

        // Every scalar value from U+0020 below U+3000: dense in the named ranges, and it skips
        // the surrogates `char::from_u32` rejects.
        let points: Vec<char> = (0..SAMPLES)
            .filter_map(|i| char::from_u32(0x20 + i))
            .filter(|c| u32::from(*c) < 0x3000)
            .collect();
        let named = u32::try_from(points.len()).expect("fewer than u32::MAX characters");

        measure("name:", named, || {
            let mut sink = 0u64;
            for c in &points {
                sink += name(*c).map_or(0, |found| found.len() as u64);
            }
            sink
        });

        measure("general_category:", ALL_CODE_POINTS, || {
            let mut sink = 0u64;
            for cp in 0..ALL_CODE_POINTS {
                let c = char::from_u32(cp).unwrap_or(' ');
                sink += general_category(c).short_name().len() as u64;
            }
            sink
        });

        // The interesting emoji shapes: canonical hit, named sequence, synthesized tone
        // variant, tone fallback with two tones, a miss, and a long tag sequence.
        let probes = ["🏳️‍🌈", "2️⃣", "👨🏻‍❤️‍💋‍👨🏻", "🫶🏻", "🧑🏽‍🦰", "🐱‍🏍", "d'être", "🏴󠁧󠁢󠁥󠁮󠁧󠁿"];
        let probe_count = u32::try_from(probes.len()).expect("a handful of probes");
        measure("emoji_name:", SAMPLES * probe_count, || {
            let mut sink = 0u64;
            for _ in 0..SAMPLES {
                for probe in probes {
                    sink += emoji_name(probe).map_or(0, |found| found.len() as u64);
                }
            }
            sink
        });
    }
}
