//! Statically known plugin construction.
//!
//! The facade's `bundled_plugins!` macro generates a single enum with one variant per
//! compiled-in plugin, so the host needs no runtime registration mechanism:
//! [`PluginSet::construct`] maps a configuration section's plugin name to the matching variant,
//! and [`LoadedPlugin`] carries the name and subscriptions the runtime routes events with.

use figment::value::Value;

use crate::Error;
use crate::config::deserialize_with_unknown_keys;
use crate::context::Context;
use crate::event::Subscriptions;
use crate::plugin::{Plugin, Registered};

/// A plugin constructed from its configuration section, with what the runtime needs to route
/// events to it.
///
/// The name comes from [`Registered::NAME`], so it can never disagree with the plugin, and the
/// subscriptions are the ones the plugin registered during construction.
pub struct LoadedPlugin<P> {
    /// The plugin's name, taken from [`Registered::NAME`].
    pub name: &'static str,
    /// The constructed plugin.
    pub plugin: P,
    /// The events the plugin registered interest in during construction.
    pub subscriptions: Subscriptions,
}

impl<P> LoadedPlugin<P> {
    /// Converts the plugin, keeping its name and subscriptions.
    ///
    /// The facade uses this to wrap each constructed plugin in its `RuntimePlugin` variant
    /// without copying the subscriptions.
    pub fn map<Q>(self, map: impl FnOnce(P) -> Q) -> LoadedPlugin<Q> {
        LoadedPlugin {
            name: self.name,
            plugin: map(self.plugin),
            subscriptions: self.subscriptions,
        }
    }
}

impl<P> LoadedPlugin<P>
where
    P: Registered,
{
    /// Constructs the plugin from its raw `[plugins.<name>]` settings section.
    ///
    /// `section` is the plugin's section with the host-managed `enabled` key removed; `None`
    /// when the section is omitted, in which case the plugin's settings take their defaults.
    ///
    /// # Errors
    ///
    /// Returns an error when the section does not deserialize into `P::Settings`, or when
    /// [`Registered::new`] fails.
    pub fn from_section(ctx: &Context, section: Option<&Value>) -> Result<Self, Error> {
        let (settings, _) = deserialize_settings::<P>(section)?;

        let mut subscriptions = Subscriptions::new();
        let plugin = P::new(ctx, &settings, &mut subscriptions)?;

        Ok(Self {
            name: P::NAME,
            plugin,
            subscriptions,
        })
    }
}

/// Deserializes a plugin's settings section, or the defaults when it is omitted.
fn deserialize_settings<P>(section: Option<&Value>) -> Result<(P::Settings, Vec<String>), Error>
where
    P: Registered,
{
    match section {
        Some(value) => {
            let (settings, unknown_keys) =
                deserialize_with_unknown_keys(value).map_err(|error| {
                    Error::Message(format!(
                        "invalid settings for plugin `{}`: {error}",
                        P::NAME
                    ))
                })?;

            Ok((settings, unknown_keys))
        }
        // No section at all: the plugin runs with its defaults. `enabled_by_default` has
        // already decided whether it is enabled.
        None => Ok((P::Settings::default(), Vec::new())),
    }
}

/// Deserializes a plugin's settings section and returns the unrecognized keys.
///
/// The facade's generated [`PluginSet::validate`] calls this while the configuration is loaded,
/// before it decides whether a plugin is enabled, so a malformed section fails startup even when
/// its plugin is disabled.
///
/// # Errors
///
/// Returns an error when the section does not deserialize into `P::Settings`.
#[doc(hidden)]
pub fn validate_settings<P>(section: Option<&Value>) -> Result<Vec<String>, Error>
where
    P: Registered,
{
    deserialize_settings::<P>(section).map(|(_, unknown_keys)| unknown_keys)
}

/// The statically known set of plugins a host can construct.
///
/// Implemented by the facade's generated enum: every compiled-in bundled plugin is one variant,
/// so construction is a name match instead of a registered constructor thunk, and the runtime
/// dispatches to a concrete type without a `dyn Plugin` object. The trait is implemented once,
/// in `crates/plugins`, and consumed by `zeta-plugin-runtime`'s registry.
pub trait PluginSet: Plugin + Sized {
    /// Constructs the plugin named `name` from its raw `[plugins.<name>]` settings section.
    ///
    /// Returns `Ok(None)` when no compiled-in plugin has that name — a bundled plugin whose
    /// cargo feature is disabled, or a section the configuration layer already reported as
    /// unknown.
    ///
    /// # Errors
    ///
    /// Returns an error when the section does not deserialize into the plugin's settings, or
    /// when the plugin's constructor fails.
    fn construct(
        ctx: &Context,
        name: &str,
        section: Option<&Value>,
    ) -> Result<Option<LoadedPlugin<Self>>, Error>;

    /// Validates the settings section of the plugin named `name` without constructing it.
    ///
    /// Returns the keys the plugin's settings type did not recognize, or `None` when no
    /// compiled-in plugin has that name. Called while the configuration is loaded so invalid
    /// settings are rejected even when the plugin is disabled.
    ///
    /// # Errors
    ///
    /// Returns an error when the section does not deserialize into the plugin's settings.
    fn validate(name: &str, section: Option<&Value>) -> Option<Result<Vec<String>, Error>>;
}

#[cfg(test)]
mod tests {
    use figment::value::Dict;
    use serde::Deserialize;
    use zeta_test_support::assert_empty;

    use super::*;

    /// A settings type with one optional key, so a typo is distinguishable from a bad value.
    #[derive(Debug, Default, Deserialize)]
    struct TestSettings {
        /// Deserialized only: the tests assert on the diagnostics.
        #[expect(dead_code, reason = "deserialized only; asserted through diagnostics")]
        value: Option<u32>,
    }

    /// A plugin whose construction cannot fail, so only the settings decide the outcome.
    struct TestPlugin;

    impl Plugin for TestPlugin {}

    impl Registered for TestPlugin {
        const NAME: &'static str = "test_plugin";
        type Settings = TestSettings;

        fn new(_: &Context, _: &Self::Settings, _: &mut Subscriptions) -> Result<Self, Error> {
            Ok(Self)
        }
    }

    /// Builds the context the host hands to a plugin.
    ///
    /// `zeta_test_support::test_context` cannot be used here: it depends on this crate, and the
    /// dev-dependency cycle gives the test target a separate copy of `Context`.
    fn context() -> Context {
        #[cfg(feature = "database")]
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://invalid/zeta_test")
            .expect("lazy database pool");

        Context::new(
            #[cfg(feature = "database")]
            db,
            crate::HostConfig::default(),
        )
    }

    /// Builds the `[plugins.<name>]` section value the host hands to the plugin.
    fn section(entries: &[(&str, Value)]) -> Value {
        let mut dict = Dict::new();
        for (key, value) in entries {
            dict.insert((*key).to_owned(), value.clone());
        }

        Value::from(dict)
    }

    #[test]
    fn omitted_section_has_no_unknown_keys() {
        let unknown = validate_settings::<TestPlugin>(None).expect("the defaults should validate");

        assert_empty!(unknown);
    }

    #[test]
    fn recognized_keys_have_no_unknown_keys() {
        let unknown =
            validate_settings::<TestPlugin>(Some(&section(&[("value", Value::from(42_u32))])))
                .expect("recognized settings should validate");

        assert_empty!(unknown);
    }

    #[test]
    fn typo_is_reported_as_an_unknown_key() {
        let unknown =
            validate_settings::<TestPlugin>(Some(&section(&[("valuee", Value::from(42_u32))])))
                .expect("unknown keys should not fail validation");

        assert_eq!(unknown, ["valuee"]);
    }

    #[test]
    fn invalid_value_type_names_the_plugin() {
        let error = validate_settings::<TestPlugin>(Some(&section(&[(
            "value",
            Value::from("not a number"),
        )])))
        .expect_err("a string is not a number");

        let message = error.to_string();
        assert!(message.contains("invalid settings"), "{message}");
        assert!(message.contains(TestPlugin::NAME), "{message}");
    }

    #[tokio::test]
    async fn construction_carries_the_name_and_subscriptions() {
        let loaded = LoadedPlugin::<TestPlugin>::from_section(
            &context(),
            Some(&section(&[("value", Value::from(42_u32))])),
        )
        .expect("the plugin should construct");

        assert_eq!(loaded.name, TestPlugin::NAME);
        assert_empty!(loaded.subscriptions.commands());
    }

    #[tokio::test]
    async fn map_keeps_the_name_and_subscriptions() {
        let loaded = LoadedPlugin::<TestPlugin>::from_section(&context(), None)
            .expect("the plugin should construct");

        let mapped = loaded.map(|_| 42_u8);

        assert_eq!(mapped.name, TestPlugin::NAME);
        assert_empty!(mapped.subscriptions.commands());
        assert_eq!(mapped.plugin, 42);
    }
}
