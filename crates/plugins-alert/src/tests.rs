//! Cross-cutting tests for the alert plugin.

use std::time::Duration;

use zeta_test_support::settings_tests;

use super::Settings;

settings_tests! {
    Settings,
    settings,
    default: {
        assert_eq!(settings.retry_delay, Duration::from_secs(30));
        assert_eq!(settings.sync_interval, Duration::from_mins(5));
        assert_eq!(settings.window, Duration::from_mins(15));
    }
    deserialize: {
        "retry_delay": "1m",
        "sync_interval": "2m",
        "window": "30m",
    } assert: {
        assert_eq!(settings.retry_delay, Duration::from_mins(1));
        assert_eq!(settings.sync_interval, Duration::from_mins(2));
        assert_eq!(settings.window, Duration::from_mins(30));
    }
}
