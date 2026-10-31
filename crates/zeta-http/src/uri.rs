//! Redaction of request URIs for span attributes and logs.
//!
//! [`RedactedUriRef`] renders an [`http::Uri`] in place, without the `to_string` and URL parse
//! the owned [`zeta_url::RedactedUrl`] needs. It lives outside the `emulated` module so a
//! benchmark or another client can use it without building the browser-emulated stack.

use std::fmt;

/// A request URI rendered for span attributes and logs, without its query string or userinfo.
///
/// [`zeta_url::RedactedUrl`] owns its rendering; a span field only needs the value while the
/// field is recorded, so this wrapper writes the URI's scheme, host, non-default port and path
/// directly from its components, without `Uri::to_string` or a URL parse.
#[derive(Clone, Copy)]
pub struct RedactedUriRef<'a>(&'a http::Uri);

impl<'a> RedactedUriRef<'a> {
    /// Returns `uri` as it may be logged: without its query string or userinfo.
    #[must_use]
    pub const fn from_uri(uri: &'a http::Uri) -> Self {
        Self(uri)
    }
}

impl fmt::Display for RedactedUriRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let uri = self.0;

        if let Some(scheme) = uri.scheme_str() {
            f.write_str(scheme)?;
            f.write_str(if uri.authority().is_some() {
                "://"
            } else {
                ":"
            })?;
        }

        if let Some(authority) = uri.authority() {
            // `Authority::host` excludes the userinfo the URI may carry.
            f.write_str(authority.host())?;

            if let Some(port) = uri.port_u16()
                && Some(port) != default_port(uri.scheme_str())
            {
                write!(f, ":{port}")?;
            }
        }

        f.write_str(uri.path())
    }
}

impl fmt::Debug for RedactedUriRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactedUriRef(")?;
        fmt::Display::fmt(self, f)?;
        f.write_str(")")
    }
}

/// Returns the default port of `scheme`, which the URL serialization omits.
fn default_port(scheme: Option<&str>) -> Option<u16> {
    match scheme {
        Some("http" | "ws") => Some(80),
        Some("https" | "wss") => Some(443),
        Some("ftp") => Some(21),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use zeta_url::RedactedUrl;

    use super::RedactedUriRef;

    #[test]
    fn redacted_uri_ref_matches_the_owned_form() {
        let uris = [
            "https://user:secret@api.example.com:8443/v1/weather?appid=secret",
            "https://example.com/path#fragment",
            "http://[::1]:8080/path?token=secret",
            "https://example.com:443/path?token=secret",
            "https://example.com/path/to/thing?access_token=secret",
            "ws://example.com:80/path?token=secret",
            "wss://example.com:443/path?token=secret",
            "ftp://example.com:21/path?token=secret",
            "ws://example.com:8080/path?token=secret",
            "/ddo/query?q=secret",
        ];

        for raw in uris {
            let uri: http::Uri = raw.parse().unwrap();
            let borrowed = RedactedUriRef::from_uri(&uri);
            let owned = RedactedUrl::from_unparsed(&uri.to_string());

            assert_eq!(borrowed.to_string(), owned.as_str(), "{raw}");
            assert!(!format!("{borrowed:?}").contains("secret"), "{raw}");
        }
    }

    #[test]
    fn redacted_uri_ref_drops_the_userinfo() {
        let uri: http::Uri = "https://user:secret@example.com:8443/path?token=secret"
            .parse()
            .unwrap();

        assert_eq!(
            RedactedUriRef::from_uri(&uri).to_string(),
            "https://example.com:8443/path"
        );
    }

    #[test]
    #[ignore = "performance measurement"]
    fn bench_redacted_uri_formatting() {
        use std::fmt::Write as _;
        use std::time::Instant;

        const FORMATS: u64 = 10_000_000;
        const ALLOCATION_FORMATS: u64 = 1_000_000;

        let uri: http::Uri = "https://user:secret@api.example.com:8443/v1/weather?appid=secret"
            .parse()
            .unwrap();
        let mut out = String::with_capacity(256);

        // Warm up both paths before measuring them.
        write!(out, "{}", RedactedUrl::from_unparsed(&uri.to_string())).unwrap();
        write!(out, "{}", RedactedUriRef::from_uri(&uri)).unwrap();
        out.clear();

        let start = Instant::now();

        for _ in 0..FORMATS {
            write!(out, "{}", RedactedUriRef::from_uri(&uri)).unwrap();
            out.clear();
        }

        let borrowed = start.elapsed();
        let start = Instant::now();

        for _ in 0..FORMATS {
            write!(out, "{}", RedactedUrl::from_unparsed(&uri.to_string())).unwrap();
            out.clear();
        }

        let owned = start.elapsed();

        println!(
            "RedactedUri: {FORMATS} formats, borrowed {borrowed:?} ({} ns/format), \
             owned {owned:?} ({} ns/format)",
            borrowed.as_nanos() / u128::from(FORMATS),
            owned.as_nanos() / u128::from(FORMATS),
        );

        let borrowed_allocations = allocation_counter::measure(|| {
            for _ in 0..ALLOCATION_FORMATS {
                write!(out, "{}", RedactedUriRef::from_uri(&uri)).unwrap();
                out.clear();
            }
        });
        let owned_allocations = allocation_counter::measure(|| {
            for _ in 0..ALLOCATION_FORMATS {
                write!(out, "{}", RedactedUrl::from_unparsed(&uri.to_string())).unwrap();
                out.clear();
            }
        });

        println!(
            "RedactedUri allocations: {ALLOCATION_FORMATS} formats, borrowed {} ({} B), \
             owned {} ({} B)",
            borrowed_allocations.count_total,
            borrowed_allocations.bytes_total,
            owned_allocations.count_total,
            owned_allocations.bytes_total,
        );
    }
}
