use std::str;

use url::Url;

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
pub struct ExtractUrls<'a> {
    iter: str::Split<'a, char>,
}

impl<'a> ExtractUrls<'a> {
    /// Creates a new `ExtractUrls` for the given string.
    #[inline]
    #[must_use]
    pub fn new(s: &'a str) -> ExtractUrls<'a> {
        ExtractUrls { iter: s.split(' ') }
    }
}

impl Iterator for ExtractUrls<'_> {
    type Item = Url;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let parser = Url::options();

        for word in self.iter.by_ref() {
            // Slices first 4 bytes without panicking on UTF-8 boundaries and checks ASCII case
            // in-place
            if let Some(prefix) = word.get(..4)
                && prefix.eq_ignore_ascii_case("http")
                && let Ok(url) = parser.parse(word)
            {
                return Some(url);
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
}
