//! Redaction of request URIs for span attributes and logs.
//!
//! [`RedactedUriRef`] renders an [`http::Uri`] in place, without the `to_string` and URL parse
//! the owned [`zeta_url::RedactedUrl`] needs. It is internal plumbing for the `emulated`
//! client's spans, so it is only re-exported inside the crate.

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

        // `Authority::host` excludes the userinfo the URI may carry; the shared renderer drops
        // the default port and keeps the IPv6 brackets as `http::uri` renders them.
        zeta_url::write_redacted(
            f,
            uri.scheme_str(),
            uri.authority().map(http::uri::Authority::host),
            uri.port_u16(),
            uri.path(),
        )
    }
}

impl fmt::Debug for RedactedUriRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactedUriRef(")?;
        fmt::Display::fmt(self, f)?;
        f.write_str(")")
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
    fn redacted_uri_ref_should_not_allocate() {
        use std::fmt::Write as _;

        let uri: http::Uri = "https://user:secret@api.example.com:8443/v1/weather?appid=secret"
            .parse()
            .unwrap();
        let mut out = String::with_capacity(256);
        let allocations = allocation_counter::measure(|| {
            for _ in 0..1_000 {
                out.clear();
                write!(out, "{}", RedactedUriRef::from_uri(&uri)).unwrap();
            }
        });

        assert_eq!(allocations.count_total, 0);
        assert_eq!(allocations.bytes_total, 0);
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
