//! Cross-cutting configuration tests: the full configuration, the repository `config.toml`,
//! plugin section extraction and the plugin-visible subset.
//!
//! Section-specific tests live next to their section: the IRC parsing and conversion tests in
//! `irc` and the database URL redaction test in `database`. The HTTP defaults and parsing tests
//! stay here because [`HttpConfig`] is owned by `zeta-http`.

use std::time::Duration;

use figment::{
    Figment,
    providers::{Format, Toml},
};
use zeta_plugin::Sender;
use zeta_test_support::assert_empty;

use super::*;

/// The base configuration the config tests start from.
const FULL_BASE: &str = r#"
[database]
url = "postgresql://localhost/zeta_test"

[tracing]
enabled = true

[irc]
nickname = "zeta"
hostname = "localhost"
alt_nicks = []
channels = []
"#;

#[test]
fn http_defaults_are_used_when_omitted() {
    let config: HttpConfig = Figment::new()
        .merge(Toml::string(""))
        .extract()
        .expect("could not parse http configuration");

    let defaults = HttpConfig::default();

    assert_eq!(config.timeout, defaults.timeout);
    assert_eq!(config.user_agent, defaults.user_agent);
}

#[test]
fn http_settings_parse() {
    let config: HttpConfig = Figment::new()
        .merge(Toml::string(
            "timeout = \"10s\"\nuser_agent = \"zeta/test\"\n",
        ))
        .extract()
        .expect("could not parse http configuration");

    assert_eq!(config.timeout, Duration::from_secs(10));
    assert_eq!(config.user_agent, "zeta/test");
}

#[test]
fn invalid_http_user_agent_is_rejected() {
    let toml = format!("{FULL_BASE}\n[http]\nuser_agent = \"bad\\nagent\"\n");
    let figment = Figment::new().merge(Toml::string(&toml));

    let error =
        Config::from_figment(&figment).expect_err("an invalid user agent should be rejected");

    assert!(error.to_string().contains("user_agent"), "{error}");
}

#[test]
fn debug_output_redacts_secrets() {
    let config: Config = Figment::new()
        .merge(Toml::string(
            r##"
[database]
url = "postgresql://zeta:db-secret@localhost/zeta_test"

[tracing]
enabled = false

[irc]
nickname = "zeta"
hostname = "localhost"
alt_nicks = []
channels = [{ name = "#zeta", key = "channel-secret" }]
nick_password = "nickserv-secret"
password = "server-secret"
"##,
        ))
        .extract()
        .expect("configuration should parse");

    let debug = format!("{config:?}");

    for secret in [
        "nickserv-secret",
        "server-secret",
        "channel-secret",
        "db-secret",
    ] {
        assert!(!debug.contains(secret), "`{secret}` leaked: {debug}");
    }

    // Non-secret fields stay visible, and the database URL keeps its host.
    assert!(debug.contains("localhost"), "{debug}");
    assert!(debug.contains("#zeta"), "{debug}");
    assert!(
        debug.contains("postgresql://localhost/zeta_test"),
        "{debug}"
    );
    assert!(debug.contains("REDACTED"), "{debug}");
}

/// The repository `config.toml` must parse into the full configuration: a broken edit has
/// to fail here rather than at someone's deployment.
#[test]
fn repository_config_parses() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config.toml");

    let config = Config::from_figment(&Figment::new().merge(Toml::file(path)))
        .expect("the repository config.toml should parse");

    assert_empty!(config.plugins.unknown);
}

#[test]
fn full_config_without_plugins_section_parses() {
    let config = Figment::new()
        .merge(Toml::string(FULL_BASE))
        .extract::<Config>()
        .expect("configuration without a [plugins] section should parse");

    assert_empty!(config.plugins.unknown);
}

#[test]
fn take_plugins_removes_sections() {
    let mut config = Figment::new()
        .merge(Toml::string(&format!(
            "{FULL_BASE}\n[plugins.dig]\nnameservers = [\"1.1.1.1\"]\n"
        )))
        .extract::<Config>()
        .expect("configuration should parse");

    let plugins = config.take_plugins();

    // The sections were removed from the config and returned to the caller; the plugin
    // values themselves are asserted in the plugins crate's configuration tests.
    assert_empty!(plugins.unknown);
    assert!(plugins.sections.contains_key("dig"));
    assert!(config.plugins.sections.is_empty());
}

#[test]
fn plugins_enabled_by_default_is_configurable() {
    let mut config = Figment::new()
        .merge(Toml::string(FULL_BASE))
        .merge(Toml::string("[plugins]\nenabled_by_default = false\n"))
        .extract::<Config>()
        .expect("configuration should parse");

    let plugins = config.take_plugins();

    // The toggle is host-managed; how it resolves each plugin's `enabled` is asserted in
    // the plugins crate's configuration tests.
    assert!(!plugins.enabled_by_default);
    assert_empty!(plugins.unknown);
}

#[test]
fn host_config_mirrors_the_visible_subset() {
    let config = Figment::new()
        .merge(Toml::string(FULL_BASE))
        .merge(Toml::string("[irc]\nadmin_hostmasks = [\"mk!mk@*\"]\n"))
        .extract::<Config>()
        .expect("configuration should parse");

    let host = zeta_plugin::config::HostConfig::from(&config);

    assert_eq!(host.http, config.http);

    // The host compiles the configured masks once; the plugin-visible config exposes the
    // matcher rather than the raw strings.
    let admins = host.admins();

    assert!(admins.matches(Sender::new("mk", "mk", "example.com")));
    assert!(!admins.matches(Sender::new("someone", "else", "example.com")));
}
