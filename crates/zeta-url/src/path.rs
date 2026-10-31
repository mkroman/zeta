//! Path-segment and query helpers the per-plugin URL matchers build on.

use smallvec::SmallVec;
use url::Url;

/// The number of path segments kept on the stack by [`path_segments`].
///
/// Posted URLs rarely carry more than a handful of segments, so the buffer stays off the heap
/// for the common case and spills gracefully for deep paths.
const INLINE_SEGMENTS: usize = 8;

/// Returns the path segments of `url`, with the empty segment a trailing slash produces removed.
///
/// The segments are borrowed from `url`, and up to eight of them live inline in the returned
/// buffer, so a typical URL does not allocate.
///
/// The `url` crate keeps path segments percent-encoded, and a trailing slash shows up as an empty
/// final segment, which otherwise has to be special-cased in every path parser.
#[must_use]
pub fn path_segments(url: &Url) -> Option<SmallVec<[&str; INLINE_SEGMENTS]>> {
    let mut segments: SmallVec<[&str; INLINE_SEGMENTS]> = url.path_segments()?.collect();

    if segments.last() == Some(&"") {
        segments.pop();
    }

    Some(segments)
}

/// Returns the last non-empty path segment of `url`, without allocating.
///
/// Trailing slashes produce empty segments; they are skipped, so `/a/b` and `/a/b/` both yield
/// `b`.
#[must_use]
pub fn last_path_segment(url: &Url) -> Option<&str> {
    url.path_segments()?
        .rev()
        .find(|segment| !segment.is_empty())
}

/// Returns the value of the first query parameter named `name`, if `url` has one.
#[must_use]
pub fn query_param(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// Returns `host` without its leading `www.` label, if it has one.
///
/// Hosts are compared case-insensitively elsewhere; this only normalizes the conventional
/// `www.` alias that links routinely carry.
#[must_use]
pub fn strip_www(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

/// Returns whether `segment` is non-empty and contains only ASCII digits.
#[must_use]
pub fn is_numeric_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit())
}

/// Returns whether `segment` is non-empty and contains only ASCII alphanumeric characters plus
/// any of the characters in `allowed`.
///
/// Path segments stay percent-encoded in the `url` crate, so a segment can never contain a raw
/// `/` — but it can contain percent-encoded separators such as `%2F`. Restricting a component to
/// the identifier alphabet therefore both rejects such junk and guarantees that canonical URLs
/// built from the segment (and ids passed as API query parameters) keep their meaning.
#[must_use]
pub fn is_identifier(segment: &str, allowed: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || allowed.contains(c))
}

/// Returns whether `id` starts with `prefix` followed by at least one ASCII digit, e.g. an IMDb
/// `tt1375666`.
#[must_use]
pub fn is_prefixed_numeric_id(id: &str, prefix: &str) -> bool {
    id.len() > prefix.len() && id.starts_with(prefix) && is_numeric_segment(&id[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_segments_should_drop_the_trailing_slash_segment() {
        let url = Url::parse("https://example.com/a/b/").unwrap();

        assert_eq!(path_segments(&url).unwrap().as_slice(), ["a", "b"]);

        let url = Url::parse("https://example.com/").unwrap();

        assert!(path_segments(&url).unwrap().is_empty());
    }

    #[test]
    fn last_path_segment_should_skip_trailing_slashes() {
        for url in [
            "https://example.com/a/b",
            "https://example.com/a/b/",
            "https://example.com/a/b//",
        ] {
            assert_eq!(
                last_path_segment(&Url::parse(url).unwrap()),
                Some("b"),
                "{url}"
            );
        }

        assert_eq!(
            last_path_segment(&Url::parse("https://example.com").unwrap()),
            None
        );
        assert_eq!(
            last_path_segment(&Url::parse("https://example.com/").unwrap()),
            None
        );
    }

    #[test]
    fn query_param_should_return_the_named_parameter() {
        let url = Url::parse("https://example.com/watch?v=abc&list=xyz").unwrap();

        assert_eq!(query_param(&url, "v").as_deref(), Some("abc"));
        assert_eq!(query_param(&url, "list").as_deref(), Some("xyz"));
        assert_eq!(query_param(&url, "missing"), None);
    }

    #[test]
    fn strip_www_should_remove_only_the_leading_www_label() {
        assert_eq!(strip_www("www.example.com"), "example.com");
        assert_eq!(strip_www("example.com"), "example.com");
        assert_eq!(strip_www("www."), "");
        assert_eq!(strip_www("wwwx.example.com"), "wwwx.example.com");
    }

    #[test]
    fn is_numeric_segment_should_only_accept_digits() {
        assert!(is_numeric_segment("123"));
        assert!(!is_numeric_segment(""));
        assert!(!is_numeric_segment("12a"));
        assert!(!is_numeric_segment("12%2F"));
    }

    #[test]
    fn is_identifier_should_accept_identifier_characters() {
        assert!(is_identifier("abc_123", "_-"));
        assert!(is_identifier("a-b_c", "_-"));
        assert!(is_identifier("a.b", "."));
        assert!(!is_identifier("", "_"));
        assert!(!is_identifier("ab%2Fcd", ""));
        assert!(!is_identifier("ab;cd", ""));
        assert!(!is_identifier("a/b", "_-"));
        assert!(!is_identifier("a b", "_-"));
    }

    #[test]
    fn is_prefixed_numeric_id_should_only_accept_prefixed_digits() {
        assert!(is_prefixed_numeric_id("tt1375666", "tt"));
        assert!(!is_prefixed_numeric_id("nm0000138", "tt"));
        assert!(!is_prefixed_numeric_id("tt", "tt"));
        assert!(!is_prefixed_numeric_id("ttbuster", "tt"));
        assert!(!is_prefixed_numeric_id("nmbuster", "nm"));
    }
}
