//! Classifies IMDb URLs.

use url::Url;

use crate::url::{is_prefixed_numeric_id, path_segments};

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
    if !super::URL_HOSTS.contains(&url.host_str()?) {
        return None;
    }

    let segments = path_segments(url);

    match segments.as_deref() {
        // `/title/<id>[/…]` — any sub-page of a title (e.g. photo galleries) refers to the title.
        Some(["title", id, ..]) if is_prefixed_numeric_id(id, "tt") => {
            Some(Link::Title((*id).to_string()))
        }
        // `/name/<id>[/…]`
        Some(["name", id, ..]) if is_prefixed_numeric_id(id, "nm") => {
            Some(Link::Name((*id).to_string()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url::assert_parses;

    #[test]
    fn classify_title_urls() {
        assert_parses(classify_imdb_url, &[
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
        ]);
    }

    #[test]
    fn classify_name_urls() {
        assert_parses(classify_imdb_url, &[
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
        ]);
    }

    #[test]
    fn classify_unrecognized_urls() {
        assert_parses(classify_imdb_url, &[
            ("https://www.imdb.com/chart/top/", None),
            ("https://www.imdb.com/title/", None),
            ("https://www.imdb.com/name/", None),
            ("https://www.imdb.com/name/nmbuster", None),
            ("https://example.com/title/tt1375666", None),
        ]);
    }
}
