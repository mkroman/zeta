//! Resolution of string settings from configuration and environment variables.

/// Resolves a string setting: the configured value wins, then the `env` environment variable,
/// then the default.
#[must_use]
pub fn resolve_setting(setting: Option<&str>, env: &str, default: &str) -> String {
    setting
        .map(str::to_string)
        .or_else(|| std::env::var(env).ok())
        .unwrap_or_else(|| default.to_string())
}

/// Resolves an optional string setting: the configured value wins over the `env` environment
/// variable.
///
/// Empty values count as unset, so a blank configured value or environment variable does not
/// configure the setting.
#[must_use]
pub fn resolve_optional_setting(setting: Option<&str>, env: &str) -> Option<String> {
    setting
        .map(str::to_string)
        .or_else(|| std::env::var(env).ok())
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_settings_in_priority_order() {
        assert_eq!(
            resolve_setting(Some("~meta"), "UNUSED_X_PREFIX", "reddit"),
            "~meta"
        );
        assert_eq!(resolve_setting(None, "UNUSED_X_PREFIX", "reddit"), "reddit");
    }

    #[test]
    fn resolves_optional_settings_in_priority_order() {
        assert_eq!(
            resolve_optional_setting(Some("value"), "UNUSED_ENV_VAR"),
            Some("value".to_string())
        );
        assert_eq!(resolve_optional_setting(Some(""), "UNUSED_ENV"), None);
        assert_eq!(resolve_optional_setting(None, "UNUSED_ENV"), None);
        assert_eq!(resolve_optional_setting(Some("  "), "UNUSED_ENV"), None);
    }
}
