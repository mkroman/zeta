use url::Url;

/// Map of accepted URL scheme prefixes.
///
/// Each entry associates a scheme prefix, matched case-insensitively against the start of a word,
/// with the canonical scheme it should be repaired to — or [`None`] when the prefix is already
/// canonical and the word can be parsed as-is.
pub type SchemeMap = &'static [(&'static str, Option<&'static str>)];

/// The default accepted schemes: canonical `http` and `https`.
const DEFAULT_SCHEMES: SchemeMap = &[("http", None), ("https", None)];

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

/// This trait implements a `urls()` function on stringy values that returns a [`ExtractUrls`]
/// iterator.
pub trait ExtractUrlsExt<'a> {
    /// Returns a [`ExtractUrls`] iterator.
    fn urls(&'a self) -> ExtractUrls<'a>;
}

impl<'a> ExtractUrlsExt<'a> for String {
    fn urls(&'a self) -> ExtractUrls<'a> {
        ExtractUrls::new(self)
    }
}

impl<'a> ExtractUrlsExt<'a> for str {
    fn urls(&'a self) -> ExtractUrls<'a> {
        ExtractUrls::new(self)
    }
}

/// Iterates over a list of URLs in a string.
///
/// Words are matched against a configurable set of scheme prefixes (see [`SchemeMap`]), so that
/// URLs posted with broken schemes — e.g. `ttps://example.com` — can be extracted and repaired.
pub struct ExtractUrls<'a> {
    iter: std::str::Split<'a, char>,
    schemes: SchemeMap,
    /// The length of the shortest accepted scheme, used to reject words quickly.
    min_scheme_len: usize,
}

impl<'a> ExtractUrls<'a> {
    /// Creates a new `ExtractUrls` for the given string, accepting `DEFAULT_SCHEMES`.
    #[inline]
    #[must_use]
    pub fn new(s: &'a str) -> ExtractUrls<'a> {
        ExtractUrls::with_schemes(s, DEFAULT_SCHEMES)
    }

    /// Creates a new `ExtractUrls` for the given string, accepting the given scheme prefixes.
    #[inline]
    #[must_use]
    pub fn with_schemes(s: &'a str, schemes: SchemeMap) -> ExtractUrls<'a> {
        let min_scheme_len = schemes
            .iter()
            .map(|(scheme, _)| scheme.len())
            .min()
            .unwrap_or(usize::MAX);

        ExtractUrls {
            iter: s.split(' '),
            schemes,
            min_scheme_len,
        }
    }
}

/// Returns the path segments of `url`, with the empty segment a trailing slash produces removed.
///
/// The `url` crate keeps path segments percent-encoded, and a trailing slash shows up as an empty
/// final segment, which otherwise has to be special-cased in every path parser.
#[must_use]
pub fn path_segments(url: &Url) -> Option<Vec<&str>> {
    let mut segments: Vec<&str> = url.path_segments()?.collect();

    if segments.last() == Some(&"") {
        segments.pop();
    }

    Some(segments)
}

/// Returns the value of the first query parameter named `name`, if `url` has one.
#[must_use]
pub fn query_param(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// Returns whether `segment` is non-empty and contains only ASCII digits.
#[must_use]
pub fn is_numeric_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit())
}

/// Returns whether `segment` is non-empty and contains only ASCII alphanumeric characters plus
/// any of the characters in `extra`.
///
/// Path segments stay percent-encoded in the `url` crate, so a segment can never contain a raw
/// `/` — but it can contain percent-encoded separators such as `%2F`. Restricting a component to
/// the identifier alphabet therefore both rejects such junk and guarantees that canonical URLs
/// built from the segment (and ids passed as API query parameters) keep their meaning.
#[must_use]
pub fn is_id_segment(segment: &str, extra: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.chars().any(|extra| extra == c))
}

/// Returns whether `id` starts with `prefix` followed by at least one ASCII digit, e.g. an IMDb
/// `tt1375666`.
#[must_use]
pub fn is_prefixed_numeric_segment(id: &str, prefix: &str) -> bool {
    id.len() > prefix.len() && id.starts_with(prefix) && is_numeric_segment(&id[prefix.len()..])
}

/// Returns the scheme entry that `word` starts with, preferring the longest match.
fn matched_scheme(word: &str, schemes: SchemeMap) -> Option<(&'static str, Option<&'static str>)> {
    let bytes = word.as_bytes();
    let mut matched: Option<(&'static str, Option<&'static str>)> = None;
    let mut matched_len = 0;

    for &(scheme, canonical) in schemes {
        // Only schemes longer than the current match are of interest — a matching scheme of the
        // same length would be the same scheme.
        if scheme.len() > matched_len
            && bytes
                .get(..scheme.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme.as_bytes()))
        {
            matched = Some((scheme, canonical));
            matched_len = scheme.len();
        }
    }

    matched
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

    #[test]
    fn path_segments_should_drop_the_trailing_slash_segment() {
        let url = Url::parse("https://example.com/a/b/").unwrap();

        assert_eq!(path_segments(&url), Some(vec!["a", "b"]));

        let url = Url::parse("https://example.com/").unwrap();

        assert_eq!(path_segments(&url), Some(Vec::new()));
    }

    #[test]
    fn query_param_should_return_the_named_parameter() {
        let url = Url::parse("https://example.com/watch?v=abc&list=xyz").unwrap();

        assert_eq!(query_param(&url, "v").as_deref(), Some("abc"));
        assert_eq!(query_param(&url, "list").as_deref(), Some("xyz"));
        assert_eq!(query_param(&url, "missing"), None);
    }

    #[test]
    fn is_numeric_segment_should_only_accept_digits() {
        assert!(is_numeric_segment("123"));
        assert!(!is_numeric_segment(""));
        assert!(!is_numeric_segment("12a"));
        assert!(!is_numeric_segment("12%2F"));
    }

    #[test]
    fn is_id_segment_should_accept_identifier_characters() {
        assert!(is_id_segment("abc_123", "_-"));
        assert!(is_id_segment("a.b", "."));
        assert!(!is_id_segment("", "_"));
        assert!(!is_id_segment("ab%2Fcd", ""));
        assert!(!is_id_segment("ab;cd", ""));
    }

    #[test]
    fn is_prefixed_numeric_segment_should_only_accept_prefixed_digits() {
        assert!(is_prefixed_numeric_segment("tt1375666", "tt"));
        assert!(!is_prefixed_numeric_segment("nm0000138", "tt"));
        assert!(!is_prefixed_numeric_segment("tt", "tt"));
        assert!(!is_prefixed_numeric_segment("ttbuster", "tt"));
    }

    /// Scheme map with the broken `ttp`/`ttps` variants, as used by the titles plugin.
    const TTP_SCHEMES: SchemeMap = &[
        ("http", None),
        ("https", None),
        ("ttp", Some("http")),
        ("ttps", Some("https")),
    ];

    const TEST_STRING: &str = r#"> Hetzner on X: "We've spun up an exploratory platform where you can find an experimental open-weight LLM inference API. Free, no SLAs. Test your own use cases and tell us what works and what doesn't 🫡No promises this becomes a permanent product: https://t.co/CE6YWbrTHz https://t.co/0j6aNULi86" / X"#;

    #[test]
    fn iter_should_extract_urls() {
        let urls = ExtractUrls::new(TEST_STRING);

        assert_eq!(urls.count(), 2);
    }

    #[test]
    fn str_ext_should_extract_urls() {
        let urls = TEST_STRING.urls();

        assert_eq!(urls.count(), 2);
    }

    #[test]
    fn string_ext_should_extract_urls() {
        let s = String::from(TEST_STRING);
        let urls = s.urls();

        assert_eq!(urls.count(), 2);
    }

    #[test]
    fn should_not_extract_ftp_urls() {
        let urls: Option<ExtractedUrl> = ExtractUrls::new("ftp://example.com/some/file.zip").next();

        assert!(urls.is_none());
    }

    #[test]
    fn should_repair_broken_schemes() {
        let urls: Vec<ExtractedUrl> =
            ExtractUrls::with_schemes("ttps://maero.dk ttp://example.com", TTP_SCHEMES).collect();

        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url.as_str(), "https://maero.dk/");
        assert_eq!(urls[0].repaired_from, Some("ttps"));
        assert_eq!(urls[1].url.as_str(), "http://example.com/");
        assert_eq!(urls[1].repaired_from, Some("ttp"));
    }

    #[test]
    fn should_not_repair_canonical_schemes() {
        let urls: Vec<ExtractedUrl> =
            ExtractUrls::with_schemes("https://maero.dk http://example.com", TTP_SCHEMES).collect();

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
            ExtractUrls::with_schemes("https://maero.dk", TTP_SCHEMES).collect();

        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].repaired_from, None);
    }

    #[test]
    fn should_skip_unparseable_words() {
        let urls: Vec<ExtractedUrl> = ExtractUrls::new("https:// https://maero.dk").collect();

        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url.as_str(), "https://maero.dk/");
    }
}
