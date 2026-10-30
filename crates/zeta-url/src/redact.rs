//! Redaction of URLs for logs and error messages.

use std::fmt;
use std::fmt::Write as _;

use url::Url;

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
    // The redacted form is composed from the borrowed parts rather than cloned-and-mutated:
    // this runs for every HTTP response span and every transport error, where the clone, its
    // per-mutation re-serializations and the final `to_string` were five-plus allocations for
    // one string that was about to be built anyway.
    #[must_use]
    pub fn from_url(url: &Url) -> Self {
        // A URL that cannot be a base (e.g. `mailto:`) has no authority to redact and no `//`
        // after the scheme, and a URL without a host serializes its path differently — compose
        // those the generic way: clone, strip the query and fragment, and let the userinfo
        // calls fail (there is no authority to take it from).
        if url.cannot_be_a_base() || !url.has_host() {
            let mut redacted = url.clone();

            redacted.set_query(None);
            redacted.set_fragment(None);
            let _ = redacted.set_username("");
            let _ = redacted.set_password(None);

            return Self(redacted.to_string());
        }

        // The userinfo is left out rather than removed, the port only when it is non-default
        // (`Url::port` answers `None` for the scheme's default), and the path is the encoded
        // form the URL serializes with — so the composed string matches what a clone with the
        // query, fragment and userinfo stripped serializes to.
        let mut redacted = String::with_capacity(url.as_str().len());
        redacted.push_str(url.scheme());
        redacted.push_str("://");
        redacted.push_str(url.host_str().unwrap_or_default());

        if let Some(port) = url.port() {
            let _ = write!(redacted, ":{port}");
        }

        redacted.push_str(url.path());

        Self(redacted)
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
    fn redacted_url_should_cut_a_cannot_be_a_base_url_at_its_query() {
        // A URL without an authority has no userinfo to drop; composing its redacted form from
        // scheme, host and path would turn `mailto:` into `mailto://`, so it takes the
        // clone-and-strip path instead.
        let url = Url::parse("mailto:user@example.com?subject=secret#frag").unwrap();

        assert!(url.cannot_be_a_base());
        assert_eq!(
            RedactedUrl::from_url(&url).as_str(),
            "mailto:user@example.com"
        );
    }

    #[test]
    fn redacted_url_should_drop_the_userinfo_of_a_non_special_url() {
        let url = Url::parse("foo://user:secret@example.com/path?token=secret").unwrap();

        assert_eq!(
            RedactedUrl::from_url(&url).as_str(),
            "foo://example.com/path"
        );
    }

    #[test]
    fn redacted_url_should_keep_an_ipv6_host_and_a_default_port_hidden() {
        let url = Url::parse("http://user:pass@[::1]:80/x?y=z").unwrap();

        assert_eq!(RedactedUrl::from_url(&url).as_str(), "http://[::1]/x");
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
}
