//! Classifies IMDb URLs.

use url::Url;

/// A link to an IMDb resource.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum Link {
    /// Link to a title (e.g. `/title/tt1375666`), including deep links to any of the pages of a
    /// title (e.g. `/title/tt1375666/mediaviewer/rm2694581760`).
    Title(String),
    /// Link to a person (e.g. `/name/nm0186505`).
    Name(String),
}

/// Attempts to parse the given `url` as an IMDb URL.
#[must_use]
pub fn classify_imdb_url(url: &Url) -> Option<Link> {
    if !matches!(url.host_str()?, "imdb.com" | "www.imdb.com" | "m.imdb.com") {
        return None;
    }

    let segments: Vec<&str> = url.path_segments()?.collect();

    match segments.as_slice() {
        // `/title/<id>[/…]` — any sub-page of a title (e.g. photo galleries) refers to the title.
        ["title", id, ..] if is_title_id(id) => Some(Link::Title((*id).to_string())),
        // `/name/<id>[/…]`
        ["name", id, ..] if is_name_id(id) => Some(Link::Name((*id).to_string())),
        _ => None,
    }
}

/// Checks if `id` looks like a title id (e.g. `tt1375666`).
fn is_title_id(id: &str) -> bool {
    id.len() > 2 && id.starts_with("tt") && id[2..].bytes().all(|byte| byte.is_ascii_digit())
}

/// Checks if `id` looks like a person id (e.g. `nm0186505`).
fn is_name_id(id: &str) -> bool {
    id.len() > 2 && id.starts_with("nm") && id[2..].bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_title_urls() {
        let test_cases = [
            (
                "https://www.imdb.com/title/tt1375666/",
                Some(Link::Title("tt1375666".to_string())),
            ),
            (
                "https://www.imdb.com/title/tt1375666",
                Some(Link::Title("tt1375666".to_string())),
            ),
            (
                "http://imdb.com/title/tt0111161",
                Some(Link::Title("tt0111161".to_string())),
            ),
            (
                "https://m.imdb.com/title/tt0111161/",
                Some(Link::Title("tt0111161".to_string())),
            ),
            // Deep links to sub-pages of a title refer to the title itself.
            (
                "https://www.imdb.com/title/tt1375666/mediaviewer/rm2694581760",
                Some(Link::Title("tt1375666".to_string())),
            ),
            (
                "https://www.imdb.com/title/tt0959621/?ref_=ttep_ep1",
                Some(Link::Title("tt0959621".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(classify_imdb_url(&url), expected, "for {url_str}");
        }
    }

    #[test]
    fn classify_name_urls() {
        let test_cases = [
            (
                "https://www.imdb.com/name/nm0000138/",
                Some(Link::Name("nm0000138".to_string())),
            ),
            (
                "https://www.imdb.com/name/nm0000138",
                Some(Link::Name("nm0000138".to_string())),
            ),
            (
                "https://www.imdb.com/name/nm0186505/awards",
                Some(Link::Name("nm0186505".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(classify_imdb_url(&url), expected, "for {url_str}");
        }
    }

    #[test]
    fn classify_unrecognized_urls() {
        let test_cases = [
            "https://www.imdb.com/chart/top/",
            "https://www.imdb.com/title/",
            "https://www.imdb.com/name/",
            "https://www.imdb.com/name/nmbuster",
            "https://example.com/title/tt1375666",
        ];

        for url_str in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(classify_imdb_url(&url), None, "for {url_str}");
        }
    }

    #[test]
    fn is_title_id_rejects_non_ids() {
        assert!(is_title_id("tt1375666"));
        assert!(!is_title_id("nm0000138"));
        assert!(!is_title_id("tt"));
        assert!(!is_title_id("ttbuster"));
    }

    #[test]
    fn is_name_id_rejects_non_ids() {
        assert!(is_name_id("nm0186505"));
        assert!(!is_name_id("tt1375666"));
        assert!(!is_name_id("nm"));
        assert!(!is_name_id("nmbuster"));
    }
}
