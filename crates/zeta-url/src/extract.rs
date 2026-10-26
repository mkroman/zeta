//! URL extraction from message text.
//!
//! Words are matched against a configurable set of scheme prefixes (see [`SchemeMap`]), so that
//! URLs posted with broken schemes — e.g. `ttps://example.com` — can be extracted and repaired.

use url::Url;

/// Map of accepted URL scheme prefixes.
///
/// Each entry associates a scheme prefix, matched case-insensitively against the start of a word,
/// with the canonical scheme it should be repaired to — or [`None`] when the prefix is already
/// canonical and the word can be parsed as-is.
pub type SchemeMap = &'static [(&'static str, Option<&'static str>)];

/// The default accepted schemes: canonical `http` and `https`.
///
/// Derived from the first two [`HTTP_SCHEMES`] entries so the canonical prefixes cannot drift.
const DEFAULT_SCHEMES: SchemeMap = &[HTTP_SCHEMES[0], HTTP_SCHEMES[1]];

/// The schemes used for host-side URL extraction.
///
/// Canonical `http` and `https`, plus the `ttp` and `ttps` variants that are missing their
/// leading `h` — the latter are repaired and the broken prefix reported through
/// [`ExtractedUrl::repaired_from`].
pub const HTTP_SCHEMES: SchemeMap = &[
    ("http", None),
    ("https", None),
    ("ttp", Some("http")),
    ("ttps", Some("https")),
];

/// A URL extracted from a message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractedUrl {
    /// The parsed URL, with its scheme repaired when posted with a broken variant.
    pub url: Url,
    /// The broken scheme prefix as posted, when extraction repaired it (e.g. `ttps`).
    pub repaired_from: Option<&'static str>,
}

/// Iterates over a list of URLs in a string.
///
/// Words are matched against a configurable set of scheme prefixes (see [`SchemeMap`]), so that
/// URLs posted with broken schemes — e.g. `ttps://example.com` — can be extracted and repaired.
pub struct ExtractUrls<'a> {
    iter: std::str::SplitWhitespace<'a>,
    schemes: SchemeMap,
    /// The length of the shortest accepted scheme, used to reject words quickly.
    min_scheme_len: usize,
}

impl<'a> ExtractUrls<'a> {
    /// Creates a new `ExtractUrls` for the given string, accepting `DEFAULT_SCHEMES`.
    #[inline]
    #[must_use]
    pub fn new(s: &'a str) -> Self {
        ExtractUrls::with_schemes(s, DEFAULT_SCHEMES)
    }

    /// Creates a new `ExtractUrls` for the given string, accepting the given scheme prefixes.
    #[inline]
    #[must_use]
    pub fn with_schemes(s: &'a str, schemes: SchemeMap) -> Self {
        let min_scheme_len = schemes
            .iter()
            .map(|(scheme, _)| scheme.len())
            .min()
            .unwrap_or(usize::MAX);

        ExtractUrls {
            iter: s.split_whitespace(),
            schemes,
            min_scheme_len,
        }
    }
}

/// Returns the scheme entry that `word` starts with, preferring the longest match.
fn matched_scheme(word: &str, schemes: SchemeMap) -> Option<(&'static str, Option<&'static str>)> {
    let bytes = word.as_bytes();

    schemes
        .iter()
        .copied()
        .filter(|(scheme, _)| {
            bytes
                .get(..scheme.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme.as_bytes()))
        })
        .max_by_key(|(scheme, _)| scheme.len())
}

impl Iterator for ExtractUrls<'_> {
    type Item = ExtractedUrl;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        for word in self.iter.by_ref() {
            // Most words in a message are shorter than the shortest accepted scheme, so this
            // rejects them without scanning the scheme map.
            if word.len() < self.min_scheme_len {
                continue;
            }

            let Some((matched, canonical)) = matched_scheme(word, self.schemes) else {
                continue;
            };

            // The posted scheme is broken when it maps to a canonical replacement — repair the
            // word by replacing the matched prefix.
            let parsed = canonical.map_or_else(
                || Url::parse(word).ok(),
                |scheme| {
                    let repaired = format!("{scheme}{}", &word[matched.len()..]);

                    Url::parse(&repaired).ok()
                },
            );

            if let Some(url) = parsed {
                return Some(ExtractedUrl {
                    url,
                    repaired_from: canonical.map(|_| matched),
                });
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_STRING: &str = r#"> Hetzner on X: "We've spun up an exploratory platform where you can find an experimental open-weight LLM inference API. Free, no SLAs. Test your own use cases and tell us what works and what doesn't 🫡No promises this becomes a permanent product: https://t.co/CE6YWbrTHz https://t.co/0j6aNULi86" / X"#;

    #[test]
    fn extract_urls_from_iterators_str_and_string_all_yield_the_same_urls() {
        assert_eq!(ExtractUrls::new(TEST_STRING).count(), 2);
    }

    #[test]
    fn should_not_extract_ftp_urls() {
        let urls: Option<ExtractedUrl> = ExtractUrls::new("ftp://example.com/some/file.zip").next();

        assert!(urls.is_none());
    }

    #[test]
    fn should_repair_broken_schemes() {
        let urls: Vec<ExtractedUrl> =
            ExtractUrls::with_schemes("ttps://maero.dk ttp://example.com", HTTP_SCHEMES).collect();

        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url.as_str(), "https://maero.dk/");
        assert_eq!(urls[0].repaired_from, Some("ttps"));
        assert_eq!(urls[1].url.as_str(), "http://example.com/");
        assert_eq!(urls[1].repaired_from, Some("ttp"));
    }

    #[test]
    fn should_not_repair_canonical_schemes() {
        let urls: Vec<ExtractedUrl> =
            ExtractUrls::with_schemes("https://maero.dk http://example.com", HTTP_SCHEMES)
                .collect();

        assert_eq!(urls.len(), 2);
        assert!(
            urls.iter()
                .all(|extracted| extracted.repaired_from.is_none())
        );
        assert_eq!(urls[0].url.as_str(), "https://maero.dk/");
    }

    #[test]
    fn should_match_schemes_case_insensitively() {
        let urls: Vec<ExtractedUrl> = ExtractUrls::new("HTTPS://maero.dk").collect();

        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url.as_str(), "https://maero.dk/");
    }

    #[test]
    fn broken_schemes_should_not_be_extracted_with_default_schemes() {
        let urls: Option<ExtractedUrl> = ExtractUrls::new("ttps://maero.dk").next();

        assert!(urls.is_none());
    }

    #[test]
    fn should_prefer_the_longest_scheme_match() {
        // "http" is a prefix of "https" — the longest matching scheme should win so that the URL
        // is not misreported as repaired.
        let urls: Vec<ExtractedUrl> =
            ExtractUrls::with_schemes("https://maero.dk", HTTP_SCHEMES).collect();

        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].repaired_from, None);
    }

    #[test]
    fn splits_on_any_whitespace() {
        let urls: Vec<ExtractedUrl> = ExtractUrls::new("first\thttps://maero.dk\nsecond").collect();

        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url.as_str(), "https://maero.dk/");
    }

    #[test]
    fn should_skip_unparseable_words() {
        let urls: Vec<ExtractedUrl> = ExtractUrls::new("https:// https://maero.dk").collect();

        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url.as_str(), "https://maero.dk/");
    }
}
