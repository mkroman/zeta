//! Site pattern validation and host matching.
//!
//! The covered sites are the ones an admin added with `.unwall add`, whatever unwall.app
//! itself has tested: an entry is either a hostname — matching the host and its `www.` variant —
//! or a `*.domain` wildcard, matching the bare domain and every subdomain.

/// Whether `input` looks like a hostname: dot-separated labels of alphanumerics and hyphens,
/// none empty and none starting or ending with a hyphen.
fn is_hostname(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }

    input.split('.').all(|label| {
        !label.is_empty()
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

/// Validates and normalizes a site argument: lowercased, an optional `*.` wildcard prefix, and
/// a hostname otherwise.
///
/// Returns [`None`] for anything else — paths, schemes, spaces, or empty labels.
#[must_use]
pub(super) fn normalize_site(input: &str) -> Option<String> {
    const WILDCARD: &str = "*.";

    let input = input.trim().to_lowercase();
    let wildcard = input.starts_with(WILDCARD);
    let host = input.strip_prefix(WILDCARD).unwrap_or(&input);

    is_hostname(host).then(|| {
        if wildcard {
            format!("{WILDCARD}{host}")
        } else {
            host.to_owned()
        }
    })
}

/// Whether `candidate` — a URL host or a stored site — matches `site`.
///
/// An exact entry matches the host and its `www.` variant; a `*.domain` wildcard matches the
/// bare domain and every subdomain.
#[must_use]
pub(super) fn matches_site(candidate: &str, site: &str) -> bool {
    let Some(domain) = site.strip_prefix("*.") else {
        return candidate.eq_ignore_ascii_case(site)
            || candidate
                .strip_prefix("www.")
                .is_some_and(|host| host.eq_ignore_ascii_case(site));
    };

    candidate
        .strip_suffix(domain)
        .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_sites() {
        assert_eq!(
            normalize_site("  Bloomberg.COM ").as_deref(),
            Some("bloomberg.com")
        );
        assert_eq!(
            normalize_site("*.bloomberg.com").as_deref(),
            Some("*.bloomberg.com")
        );
        assert_eq!(normalize_site(""), None);
        assert_eq!(normalize_site("bloomberg.com/path"), None);
        assert_eq!(normalize_site("https://bloomberg.com"), None);
        assert_eq!(normalize_site("bloomberg .com"), None);
        assert_eq!(normalize_site("bloomberg..com"), None);
        assert_eq!(normalize_site("-bloomberg.com"), None);
    }

    #[test]
    fn sites_match_hosts() {
        assert!(matches_site("bloomberg.com", "bloomberg.com"));
        assert!(matches_site("BLOOMBERG.com", "bloomberg.com"));
        assert!(matches_site("www.bloomberg.com", "bloomberg.com"));
        assert!(!matches_site("api.bloomberg.com", "bloomberg.com"));

        // The wildcard matches the bare domain and every subdomain.
        assert!(matches_site("bloomberg.com", "*.bloomberg.com"));
        assert!(matches_site("www.bloomberg.com", "*.bloomberg.com"));
        assert!(matches_site("foo.bar.bloomberg.com", "*.bloomberg.com"));
        assert!(!matches_site("notbloomberg.com", "*.bloomberg.com"));
    }
}