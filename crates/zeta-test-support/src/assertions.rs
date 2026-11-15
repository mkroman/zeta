//! Assertions shared by the workspace's URL matchers and plugin settings tests.
//!
//! The settings macro resolves `serde_json` through `$crate`, so a consuming crate needs no
//! `serde_json` dev-dependency of its own.

use std::collections::HashSet;

use url::Url;

/// Asserts that `parse` classifies every `input` of `cases` as its expected result.
///
/// Names the input when an assertion fails.
///
/// # Panics
///
/// Panics if a case's URL cannot be parsed or an expectation fails.
pub fn assert_parses<T: std::fmt::Debug + PartialEq>(
    parse: impl Fn(&Url) -> T,
    cases: &[(&str, T)],
) {
    for (input, expected) in cases {
        let url = Url::parse(input).expect("test URL parses");

        assert_eq!(&parse(&url), expected, "for {input}");
    }
}

/// Asserts that every advertised host has exactly one case and the parser accepts it.
///
/// `cases` pairs each advertised host with a path it must classify. Both lists must be unique
/// and have the same length, so a duplicate case cannot stand in for a missing host.
///
/// # Panics
///
/// Panics when either list has duplicates, the counts differ, a case names an unadvertised
/// host, the constructed URL does not parse, or `parse` rejects it.
pub fn assert_url_hosts_cover(
    hosts: &[&str],
    cases: &[(&str, &str)],
    parse: impl Fn(&Url) -> bool,
) {
    let mut unique_hosts = HashSet::with_capacity(hosts.len());
    for host in hosts {
        assert!(
            unique_hosts.insert(*host),
            "{host} is advertised more than once"
        );
    }

    let mut unique_cases = HashSet::with_capacity(cases.len());
    for (host, _) in cases {
        assert!(unique_cases.insert(*host), "{host} has more than one case");
    }

    assert_eq!(
        cases.len(),
        hosts.len(),
        "every advertised host needs a case"
    );

    for (host, path) in cases {
        assert!(hosts.contains(host), "{host} is not advertised");

        let url = Url::parse(&format!("https://{host}{path}")).expect("test URL parses");
        assert!(
            parse(&url),
            "{host}{path} is advertised but does not classify"
        );
    }
}

/// Generates the three settings tests shared by every plugin with a configuration section.
///
/// Expands to a `default_settings` test asserting the values produced by [`Default`], a
/// `settings_deserialize` test deserializing the given JSON object before asserting on it, and
/// a `settings_ignore_unknown_keys` test pinning that unknown keys are ignored instead of
/// rejected — removing a setting must never crash deployments that still name it.
///
/// The expansion resolves `serde_json` through `$crate`, so a consuming crate does not need a
/// `serde_json` dev-dependency of its own.
#[macro_export]
macro_rules! settings_tests {
    (
        $ty:ty, $settings:ident,
        default: { $($default:tt)* }
        deserialize: { $($json:tt)* } assert: { $($assert:tt)* }
    ) => {
        #[test]
        fn default_settings() {
            let $settings = <$ty>::default();

            $($default)*
        }

        #[test]
        fn settings_deserialize() {
            let $settings: $ty =
                $crate::__private::serde_json::from_value(
                    $crate::__private::serde_json::json!({ $($json)* })
                )
                .expect("could not deserialize settings");

            $($assert)*
        }

        #[test]
        fn settings_ignore_unknown_keys() {
            let mut value = $crate::__private::serde_json::json!({ $($json)* });
            value
                .as_object_mut()
                .expect("the settings fixture should be an object")
                .insert(
                    "setting_removed_in_this_version".to_owned(),
                    $crate::__private::serde_json::Value::Bool(true),
                );

            $crate::__private::serde_json::from_value::<$ty>(value)
                .expect("unknown settings keys must be ignored, never rejected");
        }
    };
}

/// Asserts that the expression is empty, naming the value in the panic.
///
/// A plain `assert!(value.is_empty())` only prints the condition on failure — useless for
/// finding out what the stray entries were, which is the reasoning behind clippy's
/// `assert_is_empty` lint — while the `assert_eq!` alternative needs brittle explicit types
/// (`Vec::<String>::new()`, because `[]` does not typecheck for a `Vec<String>`). This macro
/// prints the value without either problem.
///
/// An optional message follows the value, formatted like any `format!` invocation.
///
/// # Panics
///
/// Panics if the expression is not empty, printing its `Debug` rendering and, when given,
/// the custom message.
#[macro_export]
macro_rules! assert_empty {
    ($value:expr $(,)?) => {
        match &$value {
            value if value.is_empty() => {}
            value => panic!("assertion failed: value is empty\n  value: {value:?}"),
        }
    };
    ($value:expr, $($message:tt)+) => {
        match &$value {
            value if value.is_empty() => {}
            value => panic!(
                "assertion failed: value is empty\n  value: {value:?}\n{}",
                format_args!($($message)+)
            ),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::assert_url_hosts_cover;

    #[test]
    fn assert_empty_passes_on_empty_and_names_the_value_on_failure() {
        assert_empty!(Vec::<String>::new());
        let empty: [u8; 0] = [];
        assert_empty!(empty);

        let panic = std::panic::catch_unwind(|| assert_empty!(vec!["stray entry"]))
            .expect_err("non-empty values must panic");
        let message = panic
            .downcast_ref::<String>()
            .expect("the panic payload is a string");

        assert!(message.contains("value is empty"), "{message}");
        assert!(message.contains(r#""stray entry""#), "{message}");
    }

    #[test]
    fn assert_empty_appends_the_custom_message_to_the_panic() {
        let panic = std::panic::catch_unwind(|| {
            let channel = "#zeta";

            assert_empty!(vec!["stray"], "{channel} must have no strays");
        })
        .expect_err("non-empty values must panic");
        let message = panic
            .downcast_ref::<String>()
            .expect("the panic payload is a string");

        assert!(message.contains("#zeta must have no strays"), "{message}");
    }

    #[test]
    fn assert_url_hosts_cover_checks_every_advertised_host() {
        let hosts = ["example.com", "www.example.com"];
        let cases = [("example.com", "/"), ("www.example.com", "/path")];

        assert_url_hosts_cover(&hosts, &cases, |url| url.scheme() == "https");

        let panic = std::panic::catch_unwind(|| {
            assert_url_hosts_cover(&hosts, &cases[..1], |_| true);
        })
        .expect_err("a missing case must panic");
        let message = panic
            .downcast_ref::<String>()
            .expect("the panic payload is a string");

        assert!(
            message.contains("every advertised host needs a case"),
            "{message}"
        );
    }

    #[test]
    fn assert_url_hosts_cover_rejects_duplicate_hosts_and_cases() {
        let panic = std::panic::catch_unwind(|| {
            assert_url_hosts_cover(
                &["example.com", "example.com"],
                &[("example.com", "/")],
                |_| true,
            );
        })
        .expect_err("a duplicate advertised host must panic");
        let message = panic
            .downcast_ref::<String>()
            .expect("the panic payload is a string");

        assert!(message.contains("advertised more than once"), "{message}");

        let panic = std::panic::catch_unwind(|| {
            assert_url_hosts_cover(
                &["example.com", "www.example.com"],
                &[("example.com", "/"), ("example.com", "/path")],
                |_| true,
            );
        })
        .expect_err("a duplicate case must panic");
        let message = panic
            .downcast_ref::<String>()
            .expect("the panic payload is a string");

        assert!(message.contains("has more than one case"), "{message}");
    }
}
