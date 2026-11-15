//! Object keys and public links for mirrored media.
//!
//! Mirrored files are uploaded under a plugin-owned key prefix, and each mirrored file is linked
//! through the fragment of the plugin's public URL base. Both conventions are shared by the
//! service's fast-path check, the download manager and its upload pipeline, so they live here.

use url::Url;

/// Returns the object key for the given file name under `prefix`.
#[must_use]
pub fn object_key(prefix: &str, filename: &str) -> String {
    match prefix.trim_end_matches('/') {
        "" => filename.to_string(),
        prefix => format!("{prefix}/{filename}"),
    }
}

/// Returns the public URL for the given id, using the given base URL.
#[must_use]
pub fn public_url_for(base: &Url, id: &str) -> String {
    let mut url = base.clone();
    url.set_fragment(Some(id));
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_key() {
        assert_eq!(object_key("", "123.mp4"), "123.mp4");
        assert_eq!(object_key("reddit", "123.mp4"), "reddit/123.mp4");
        assert_eq!(
            object_key("~meta/reddit", "123.mp4"),
            "~meta/reddit/123.mp4"
        );
        assert_eq!(
            object_key("~meta/reddit/", "123.mp4"),
            "~meta/reddit/123.mp4"
        );
    }

    #[test]
    fn test_public_url_for() {
        let base = Url::parse("https://pub.rwx.im/reddit").unwrap();

        assert_eq!(
            public_url_for(&base, "pxtf7mx2xqzg1"),
            "https://pub.rwx.im/reddit#pxtf7mx2xqzg1"
        );
    }
}
