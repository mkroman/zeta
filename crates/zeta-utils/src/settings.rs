//! Resolution of string settings from configuration and environment variables.

/// Resolves a string setting: the configured value wins, then the `env` environment variable,
/// then the default.
///
/// Unlike [`resolve_optional_setting`], a blank configured value counts as set and is returned;
/// this keeps a deliberately empty setting empty instead of silently restoring a default.
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
/// Empty values count as unset on both sides: a blank configured value falls back to the
/// environment variable, and a blank environment variable does not configure the setting.
#[must_use]
pub fn resolve_optional_setting(setting: Option<&str>, env: &str) -> Option<String> {
    let environment = std::env::var(env).ok();

    resolve_optional_setting_with(setting, environment.as_deref())
}

/// Resolves an optional setting from the configured value and an already-read environment value.
///
/// Kept separate from [`resolve_optional_setting`] so the precedence and blank handling can be
/// tested without mutating the process environment.
fn resolve_optional_setting_with(
    setting: Option<&str>,
    environment: Option<&str>,
) -> Option<String> {
    setting
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| environment.map(str::to_string))
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
    fn optional_settings_prefer_the_configured_value() {
        assert_eq!(
            resolve_optional_setting_with(Some("configured"), Some("environment")),
            Some("configured".to_string())
        );
        assert_eq!(
            resolve_optional_setting_with(Some("configured"), None),
            Some("configured".to_string())
        );
    }

    #[test]
    fn blank_configured_optional_settings_fall_back_to_the_environment() {
        assert_eq!(
            resolve_optional_setting_with(Some(""), Some("environment")),
            Some("environment".to_string())
        );
        assert_eq!(
            resolve_optional_setting_with(Some("  "), Some("environment")),
            Some("environment".to_string())
        );
        assert_eq!(
            resolve_optional_setting_with(None, Some("environment")),
            Some("environment".to_string())
        );
    }

    #[test]
    fn blank_environment_values_do_not_configure_the_setting() {
        assert_eq!(resolve_optional_setting_with(Some(""), Some("")), None);
        assert_eq!(resolve_optional_setting_with(Some("  "), Some("  ")), None);
        assert_eq!(resolve_optional_setting_with(None, Some("")), None);
        assert_eq!(resolve_optional_setting_with(None, None), None);
    }

    #[test]
    fn public_optional_resolution_uses_an_unset_environment() {
        assert_eq!(
            resolve_optional_setting(Some("value"), "UNUSED_ENV_VAR"),
            Some("value".to_string())
        );
        assert_eq!(resolve_optional_setting(Some(""), "UNUSED_ENV"), None);
        assert_eq!(resolve_optional_setting(None, "UNUSED_ENV"), None);
        assert_eq!(resolve_optional_setting(Some("  "), "UNUSED_ENV"), None);
    }
}
