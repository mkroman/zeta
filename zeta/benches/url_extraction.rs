//! Benchmarks for various implementations of URL extraction from string slices.

use url::Url;

const TEST_MESSAGE: &str = "> Hetzner on X: \"We've spun up an exploratory platform where you can find an experimental open-weight LLM inference API. Free, no SLAs. Test your own use cases and tell us what works and what doesn't 🫡No promises this becomes a permanent product: https://t.co/CE6YWbrTHz https://t.co/0j6aNULi86\" / X";

fn main() {
    divan::main();
}

#[divan::bench]
fn string_and_vec_url_allocations() {
    fn extract_urls(s: &str) -> Option<Vec<Url>> {
        let urls: Vec<Url> = s
            .split(' ')
            .filter(|word| word.to_ascii_lowercase().starts_with("http"))
            .filter_map(|word| Url::parse(word).ok())
            .collect();

        (!urls.is_empty()).then_some(urls)
    }
    let urls = extract_urls(TEST_MESSAGE);

    assert_eq!(urls.unwrap().len(), 2);
}

#[divan::bench]
fn vec_url_allocations() {
    fn extract_urls(s: &str) -> Option<Vec<Url>> {
        let urls: Vec<Url> = s
            .split(' ')
            .filter(|word| {
                word.get(0..4)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http"))
            })
            .filter_map(|word| Url::parse(word).ok())
            .collect();

        (!urls.is_empty()).then_some(urls)
    }
    let urls = extract_urls(TEST_MESSAGE);

    assert_eq!(urls.unwrap().len(), 2);
}

#[divan::bench]
fn custom_iter_allocating_urls() {
    struct ExtractUrls<'a> {
        iter: std::str::Split<'a, char>,
    }

    impl<'a> ExtractUrls<'a> {
        #[inline]
        pub fn new(s: &'a str) -> Self {
            Self { iter: s.split(' ') }
        }
    }

    impl Iterator for ExtractUrls<'_> {
        type Item = Url;

        #[inline]
        fn next(&mut self) -> Option<Self::Item> {
            for word in self.iter.by_ref() {
                // Slices first 4 bytes without panicking on UTF-8 boundaries and checks ASCII case in-place
                if let Some(prefix) = word.get(..4)
                    && prefix.eq_ignore_ascii_case("http")
                    && let Ok(url) = Url::parse(word)
                {
                    return Some(url);
                }
            }

            None
        }
    }
    let urls = ExtractUrls::new(TEST_MESSAGE);

    assert_eq!(urls.count(), 2);
}

#[divan::bench]
fn custom_iter_allocating_urls_options() {
    struct ExtractUrls<'a> {
        iter: std::str::Split<'a, char>,
    }

    impl<'a> ExtractUrls<'a> {
        #[inline]
        pub fn new(s: &'a str) -> Self {
            Self { iter: s.split(' ') }
        }
    }

    impl Iterator for ExtractUrls<'_> {
        type Item = Url;

        #[inline]
        fn next(&mut self) -> Option<Self::Item> {
            for word in self.iter.by_ref() {
                // Slices first 4 bytes without panicking on UTF-8 boundaries and checks ASCII case in-place
                if let Some(prefix) = word.get(..4)
                    && prefix.eq_ignore_ascii_case("http")
                    && let Ok(url) = Url::options().parse(word)
                {
                    return Some(url);
                }
            }

            None
        }
    }
    let urls = ExtractUrls::new(TEST_MESSAGE);

    assert_eq!(urls.count(), 2);
}

#[divan::bench]
fn custom_iter_allocating_urls_reuse_options() {
    struct ExtractUrls<'a> {
        iter: std::str::Split<'a, char>,
    }

    impl<'a> ExtractUrls<'a> {
        #[inline]
        pub fn new(s: &'a str) -> Self {
            Self { iter: s.split(' ') }
        }
    }

    impl Iterator for ExtractUrls<'_> {
        type Item = Url;

        #[inline]
        fn next(&mut self) -> Option<Self::Item> {
            let parser = Url::options();

            for word in self.iter.by_ref() {
                // Slices first 4 bytes without panicking on UTF-8 boundaries and checks ASCII case in-place
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
    let urls = ExtractUrls::new(TEST_MESSAGE);

    assert_eq!(urls.count(), 2);
}

#[divan::bench]
fn custom_iter_urls_reuse_options() {
    struct ExtractUrls<'a> {
        iter: std::str::Split<'a, char>,
    }

    impl<'a> ExtractUrls<'a> {
        #[inline]
        pub fn new(s: &'a str) -> Self {
            Self { iter: s.split(' ') }
        }
    }

    impl Iterator for ExtractUrls<'_> {
        type Item = Url;

        #[inline]
        fn next(&mut self) -> Option<Self::Item> {
            let parser = Url::options();

            for word in self.iter.by_ref() {
                // Slices first 4 bytes without panicking on UTF-8 boundaries and checks ASCII case in-place
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
    let urls = ExtractUrls::new(TEST_MESSAGE);

    assert_eq!(urls.count(), 2);
}
