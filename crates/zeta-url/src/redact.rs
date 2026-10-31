//! Redaction of URLs for logs and error messages.

use std::fmt;

use url::{Position, Url};

/// A URL rendered for logs and error messages, without its query string, fragment, or userinfo.
///
/// The API keys this bot uses travel in the query string — and a proxy URL may carry one as
/// `user:password@host` — while a URL reaches logs, OpenTelemetry `url.full` attributes and,
/// through a plugin's error message, the channel. Which parameter holds the credential differs
/// per API and cannot be recognized reliably, so the query is dropped outright; the scheme,
/// host, path and port still say which resource was asked for.
///
/// The raw URL is never stored, and [`std::fmt::Display`] and [`Debug`] both render the redacted
/// form, so a value of this type can be logged without a separate redaction call:
///
/// ```
/// # use url::Url;
/// # use zeta_url::RedactedUrl;
/// let url = Url::parse("https://user:secret@example.com/path?token=secret").unwrap();
/// let redacted = RedactedUrl::from_url(&url);
///
/// assert_eq!(redacted.to_string(), "https://example.com/path");
/// assert_eq!(
///     format!("{redacted:?}"),
///     r#"RedactedUrl("https://example.com/path")"#
/// );
/// ```
///
/// Constructing one is a named conversion rather than a `From<Url>` implementation: redaction
/// discards the query, fragment and userinfo, and `From` is reserved for infallible, lossless
/// conversions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactedUrl(String);

impl RedactedUrl {
    /// Returns `url` as it may be logged: without its query string, fragment, or userinfo.
    #[must_use]
    pub fn from_url(url: &Url) -> Self {
        let mut redacted = url.clone();

        redacted.set_query(None);
        redacted.set_fragment(None);

        // Neither call fails here: only a URL that cannot be a base has no authority to take
        // the userinfo from, and such a URL cannot be the target of a request.
        let _ = redacted.set_username("");
        let _ = redacted.set_password(None);

        Self(redacted.to_string())
    }

    /// Returns a request URL held as text as it may be logged.
    ///
    /// The string form of [`RedactedUrl::from_url`] for URLs a client exposes as text — a
    /// `wreq::Uri`, for example. An absolute URL is parsed as-is; a scheme-relative reference is
    /// parsed against a synthetic scheme so the `url` crate strips its userinfo. An origin-form
    /// URI has no authority and therefore no userinfo, so only its query and fragment are
    /// dropped.
    #[must_use]
    pub fn from_unparsed(url: &str) -> Self {
        if let Ok(parsed) = Url::parse(url) {
            return Self::from_url(&parsed);
        }

        // A scheme-relative URL (`//user:secret@example.com/path`) is a valid reference but not
        // an absolute URL; parse it against a synthetic scheme and drop that scheme again.
        if url.starts_with("//")
            && let Ok(parsed) = Url::parse(&format!("https:{url}"))
        {
            let redacted = Self::from_url(&parsed);
            let relative = redacted
                .0
                .strip_prefix("https:")
                .unwrap_or(&redacted.0)
                .to_string();

            return Self(relative);
        }

        // An origin-form URI has no authority, and therefore no userinfo: the query and
        // fragment are all that is left to drop.
        Self(url.split(['?', '#']).next().unwrap_or(url).to_string())
    }

    /// Returns the redacted URL as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RedactedUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A borrowed [`RedactedUrl`], for rendering a URL without owning a `String`.
///
/// [`RedactedUrl`] owns its rendering because an error message carries it past the call that
/// built it. A span field or log line only needs the value while the field is recorded, so this
/// wrapper renders the same redacted form — scheme, host, non-default port and path, without the
/// userinfo, query or fragment — straight from the URL's components, without cloning or
/// re-serializing it:
///
/// ```
/// # use url::Url;
/// # use zeta_url::{RedactedUrl, RedactedUrlRef};
/// let url = Url::parse("https://user:secret@example.com/path?token=secret").unwrap();
///
/// assert_eq!(
///     RedactedUrlRef::from_url(&url).to_string(),
///     RedactedUrl::from_url(&url).as_str()
/// );
/// ```
#[derive(Clone, Copy)]
pub struct RedactedUrlRef<'a>(&'a Url);

impl<'a> RedactedUrlRef<'a> {
    /// Returns `url` as it may be logged: without its query string, fragment, or userinfo.
    #[must_use]
    pub const fn from_url(url: &'a Url) -> Self {
        Self(url)
    }
}

impl fmt::Display for RedactedUrlRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let url = self.0;

        // Write the serialization piecewise, skipping the pieces `RedactedUrl` clears. The slice
        // between the scheme and the username is `://` for a URL with an authority and `:`
        // otherwise; the host slice keeps IPv6 brackets, and the slice up to the port's end is
        // empty unless the URL carries a non-default port — exactly as `Url` serializes them.
        f.write_str(&url[Position::BeforeScheme..Position::AfterScheme])?;
        f.write_str(&url[Position::AfterScheme..Position::BeforeUsername])?;
        f.write_str(&url[Position::BeforeHost..Position::AfterHost])?;
        f.write_str(&url[Position::AfterHost..Position::AfterPort])?;
        f.write_str(&url[Position::BeforePath..Position::AfterPath])
    }
}

impl fmt::Debug for RedactedUrlRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactedUrlRef(\"")?;
        fmt::Display::fmt(self, f)?;
        f.write_str("\")")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_url_should_drop_the_query_string() {
        let url = Url::parse("https://api.example.com/v1/weather?q=horsens&appid=secret").unwrap();

        assert_eq!(
            RedactedUrl::from_url(&url).as_str(),
            "https://api.example.com/v1/weather"
        );
    }

    #[test]
    fn redacted_url_should_drop_the_userinfo() {
        let url = Url::parse("https://user:secret@example.com:8443/path?token=secret").unwrap();

        assert_eq!(
            RedactedUrl::from_url(&url).as_str(),
            "https://example.com:8443/path"
        );
    }

    #[test]
    fn redacted_url_should_drop_the_fragment() {
        let url = Url::parse("https://example.com/path#access_token=secret").unwrap();

        assert_eq!(
            RedactedUrl::from_url(&url).as_str(),
            "https://example.com/path"
        );
    }

    #[test]
    fn redacted_url_should_keep_a_url_without_credentials_as_it_is() {
        let url = Url::parse("https://example.com/path/to/thing").unwrap();

        assert_eq!(RedactedUrl::from_url(&url).as_str(), url.as_str());
    }

    #[test]
    fn redacted_url_should_render_the_same_for_display_and_debug() {
        let url = Url::parse("https://example.com/path?token=secret").unwrap();
        let redacted = RedactedUrl::from_url(&url);

        assert_eq!(redacted.to_string(), "https://example.com/path");
        assert_eq!(
            format!("{redacted:?}"),
            r#"RedactedUrl("https://example.com/path")"#
        );
        assert!(!format!("{redacted:?}").contains("secret"));
    }

    #[test]
    fn redacted_url_from_unparsed_should_cut_an_unparseable_url_at_its_query() {
        assert_eq!(
            RedactedUrl::from_unparsed("/ddo/query?q=secret").as_str(),
            "/ddo/query"
        );
        assert_eq!(
            RedactedUrl::from_unparsed("https://example.com/path?token=secret").as_str(),
            "https://example.com/path"
        );
    }

    #[test]
    fn redacted_url_from_unparsed_should_drop_userinfo_from_an_authority() {
        assert_eq!(
            RedactedUrl::from_unparsed("//user:secret@example.com/path?token=secret").as_str(),
            "//example.com/path"
        );
    }

    #[test]
    fn redacted_url_ref_should_match_the_owned_form() {
        let urls = [
            "https://api.example.com/v1/weather?q=horsens&appid=secret",
            "https://user:secret@example.com:8443/path?token=secret",
            "https://example.com/path#access_token=secret",
            "http://[::1]:8080/path?token=secret",
            "https://example.com:443/path?token=secret",
            "https://example.com",
            "file:///tmp/with%20space",
            "mailto:user@example.com?subject=secret",
        ];

        for raw in urls {
            let url = Url::parse(raw).unwrap();
            let borrowed = RedactedUrlRef::from_url(&url);
            let owned = RedactedUrl::from_url(&url);

            assert_eq!(borrowed.to_string(), owned.as_str(), "{raw}");
        }
    }

    #[test]
    fn redacted_url_ref_should_drop_the_secrets_from_display_and_debug() {
        let url =
            Url::parse("https://user:secret@example.com:8443/path?token=secret#fragment").unwrap();
        let redacted = RedactedUrlRef::from_url(&url);

        assert_eq!(redacted.to_string(), "https://example.com:8443/path");
        assert_eq!(
            format!("{redacted:?}"),
            r#"RedactedUrlRef("https://example.com:8443/path")"#
        );
        assert!(!format!("{redacted:?}").contains("secret"));
    }

    #[test]
    #[ignore = "performance measurement"]
    fn bench_redacted_url_formatting() {
        use std::fmt::Write as _;
        use std::time::Instant;

        const FORMATS: u64 = 10_000_000;

        let url =
            Url::parse("https://user:secret@api.example.com:8443/v1/weather?appid=secret#frag")
                .unwrap();
        let mut out = String::with_capacity(256);

        // Warm up both paths before measuring them.
        write!(out, "{}", RedactedUrl::from_url(&url)).unwrap();
        write!(out, "{}", RedactedUrlRef::from_url(&url)).unwrap();
        out.clear();

        let start = Instant::now();

        for _ in 0..FORMATS {
            write!(out, "{}", RedactedUrlRef::from_url(&url)).unwrap();
            out.clear();
        }

        let borrowed = start.elapsed();
        let start = Instant::now();

        for _ in 0..FORMATS {
            write!(out, "{}", RedactedUrl::from_url(&url)).unwrap();
            out.clear();
        }

        let owned = start.elapsed();

        println!(
            "RedactedUrl: {FORMATS} formats, borrowed {borrowed:?} ({} ns/format), \
             owned {owned:?} ({} ns/format)",
            borrowed.as_nanos() / u128::from(FORMATS),
            owned.as_nanos() / u128::from(FORMATS),
        );
    }
}
