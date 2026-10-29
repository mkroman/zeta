//! A plugin's handle to the shared mirror.

use std::sync::Arc;

use tracing::warn;
use url::Url;

use crate::{Mirror, MirrorError};

/// A plugin's handle to the shared [`Mirror`].
///
/// Carries the key prefix the plugin's mirrored files are uploaded under and the base URL its
/// public links are built from.
#[derive(Clone)]
pub struct MirrorHandle {
    /// The shared mirror.
    mirror: Arc<Mirror>,
    /// The key prefix for the plugin's uploads.
    prefix: String,
    /// The base URL for the plugin's public links.
    public_url_base: Url,
}

impl MirrorHandle {
    /// Creates a handle for the given mirror.
    #[must_use]
    pub const fn new(mirror: Arc<Mirror>, prefix: String, public_url_base: Url) -> Self {
        Self {
            mirror,
            prefix,
            public_url_base,
        }
    }

    /// Resolves a handle from a plugin's configuration.
    ///
    /// Every plugin follows the same convention: the download prefix resolves through the
    /// `<PLUGIN>_S3_PREFIX` environment variable and defaults to the plugin name; the public
    /// URL base resolves through `<PLUGIN>_PUBLIC_URL_BASE` and defaults to
    /// `https://pub.rwx.im/<plugin>`.
    ///
    /// Returns `None` when the shared mirror is unavailable or the configured public URL base is
    /// invalid, in which case the plugin should degrade to its non-mirroring behavior.
    #[must_use]
    pub fn resolve(
        mirror: Option<Arc<Mirror>>,
        plugin: &str,
        prefix: Option<&str>,
        public_url_base: Option<&str>,
    ) -> Option<Self> {
        let mirror = mirror?;
        let uppercase = plugin.to_ascii_uppercase();
        let public_url_base = resolve_public_url_base(
            public_url_base,
            &format!("{uppercase}_PUBLIC_URL_BASE"),
            &format!("https://pub.rwx.im/{plugin}"),
        )?;
        let prefix = zeta_utils::settings::resolve_setting(
            prefix,
            &format!("{uppercase}_S3_PREFIX"),
            plugin,
        );

        Some(Self::new(mirror, prefix, public_url_base))
    }

    /// Mirrors the media at `url` unless it has already been mirrored or a mirror of it is
    /// already in progress; see [`Mirror::ensure_mirrored`].
    ///
    /// # Errors
    ///
    /// Returns an error if it could not be checked whether the media has already been mirrored.
    pub async fn ensure_mirrored<F>(
        &self,
        url: &str,
        id: &str,
        on_mirrored: F,
    ) -> Result<Option<String>, MirrorError>
    where
        F: FnOnce(String) + Send + 'static,
    {
        self.mirror
            .ensure_mirrored(&self.prefix, &self.public_url_base, url, id, on_mirrored)
            .await
    }
}

/// Resolves the public URL base from a plugin's configuration.
///
/// The configured value wins, then the environment variable, then the default. Returns `None` and
/// logs a warning when the resolved value is not a valid URL.
fn resolve_public_url_base(setting: Option<&str>, env: &str, default: &str) -> Option<Url> {
    let value = zeta_utils::settings::resolve_setting(setting, env, default);

    match Url::parse(&value) {
        Ok(url) => Some(url),
        Err(error) => {
            warn!(%error, %value, "invalid public url base, mirroring is disabled");

            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_public_url_base() {
        let base = resolve_public_url_base(
            Some("https://pub.example.com/x"),
            "X_PUBLIC_URL_BASE",
            "https://pub.rwx.im/reddit",
        )
        .expect("configured base should be valid");

        assert_eq!(base.as_str(), "https://pub.example.com/x");

        let base = resolve_public_url_base(None, "X_PUBLIC_URL_BASE", "https://pub.rwx.im/reddit")
            .expect("default base should be valid");

        assert_eq!(base.as_str(), "https://pub.rwx.im/reddit");

        assert!(resolve_public_url_base(Some("not a url"), "X_PUBLIC_URL_BASE", "").is_none());
    }
}
