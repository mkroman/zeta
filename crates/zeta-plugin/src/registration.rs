//! Self-registration of plugins through the [`inventory`] crate.
//!
//! Plugins written with the `#[plugin]` attribute macro submit a [`PluginRegistration`] while
//! the crate is linked, so the host can discover every compiled-in plugin without a central
//! list. The registration carries the plugin's name and a monomorphized constructor thunk that
//! deserializes the plugin's `[plugins.<name>]` settings and builds the plugin on demand.

use figment::value::Value;

use crate::Error;
use crate::config::deserialize_with_unknown_keys;
use crate::context::Context;
use crate::erased::ErasedPlugin;
use crate::error::plugin_err;
use crate::event::Subscriptions;
use crate::plugin::Plugin;

inventory::collect!(PluginRegistration);

/// A plugin made available to the host for registration.
///
/// The `#[plugin]` attribute macro emits an [`inventory::submit!`] for one of these next to the
/// generated [`Plugin`](crate::Plugin) implementation. The host collects them with
/// [`inventory::iter`] and hands each one to its registry; see `zeta-plugin-runtime`.
pub struct PluginRegistration {
    /// The plugin's name (`Plugin::NAME`), used as its `[plugins.<name>]` section and in the
    /// plugin catalog.
    pub name: &'static str,
    /// Deserializes the plugin's settings section, if any, and constructs the plugin.
    ///
    /// Called by the host registry; plugins never call it themselves.
    #[doc(hidden)]
    pub construct: fn(&Context, Option<&Value>) -> Result<LoadedPlugin, Error>,
    /// Deserializes the plugin's settings section and reports the unrecognized keys.
    ///
    /// Called by the host while loading the configuration, before it decides whether the plugin
    /// is enabled, so a malformed section fails startup even when its plugin is disabled.
    validate: fn(Option<&Value>) -> Result<Vec<String>, Error>,
}

impl PluginRegistration {
    /// Constructs the registration for the plugin type `P`.
    ///
    /// The plugin's name comes from [`Plugin::NAME`](crate::Plugin::NAME), so the registration and
    /// the plugin can never disagree on it. The `#[plugin]` attribute macro calls this in the
    /// [`inventory::submit!`] it generates; plugins never call it themselves. The thunk this
    /// builds returns an error when the plugin's settings section does not deserialize into
    /// `P::Settings`, or when [`Plugin::new`](crate::Plugin::new) fails.
    #[must_use]
    pub const fn new<P>() -> Self
    where
        P: Plugin + 'static,
    {
        Self {
            name: P::NAME,
            construct: construct::<P>,
            validate: validate_settings::<P>,
        }
    }

    /// Deserializes `section` into the plugin's settings type without constructing the plugin.
    ///
    /// Returns the keys the settings type did not recognize. The host calls this while loading
    /// the configuration so invalid settings are rejected before the enabled decision.
    ///
    /// # Errors
    ///
    /// Returns an error when the section does not deserialize into `P::Settings`.
    #[doc(hidden)]
    pub fn validate_settings(&self, section: Option<&Value>) -> Result<Vec<String>, Error> {
        (self.validate)(section)
    }
}

/// A plugin loaded from its registration, with the subscriptions the registry needs to route
/// events to it.
pub struct LoadedPlugin {
    /// The plugin's name, taken from [`Plugin::NAME`](crate::Plugin::NAME) so it can never
    /// disagree with the registration.
    pub name: &'static str,
    /// The constructed plugin, with its settings type erased.
    pub plugin: Box<dyn ErasedPlugin>,
    /// The events the plugin registered interest in during construction.
    pub subscriptions: Subscriptions,
}

/// The constructor thunk [`PluginRegistration::new`] stores: deserialize the settings section,
/// build the plugin, and return it with its declared subscriptions.
fn construct<P>(ctx: &Context, section: Option<&Value>) -> Result<LoadedPlugin, Error>
where
    P: Plugin + 'static,
{
    let (settings, _) = deserialize_settings::<P>(section)?;

    let mut subscriptions = Subscriptions::new();
    let plugin = P::new(ctx, &settings, &mut subscriptions)?;

    Ok(LoadedPlugin {
        name: P::NAME,
        plugin: Box::new(plugin),
        subscriptions,
    })
}

/// Deserializes a plugin's settings section, or the defaults when it is omitted.
fn deserialize_settings<P>(section: Option<&Value>) -> Result<(P::Settings, Vec<String>), Error>
where
    P: Plugin + 'static,
{
    match section {
        Some(value) => {
            let (settings, unknown_keys) =
                deserialize_with_unknown_keys(value).map_err(|error| {
                    plugin_err(std::io::Error::other(format!(
                        "invalid settings for plugin `{}`: {error}",
                        P::NAME
                    )))
                })?;

            Ok((settings, unknown_keys))
        }
        // No section at all: the plugin runs with its defaults. `enabled_by_default` has
        // already decided whether it is enabled.
        None => Ok((P::Settings::default(), Vec::new())),
    }
}

/// Deserializes a plugin's settings section and returns only the unrecognized keys.
fn validate_settings<P>(section: Option<&Value>) -> Result<Vec<String>, Error>
where
    P: Plugin + 'static,
{
    deserialize_settings::<P>(section).map(|(_, unknown_keys)| unknown_keys)
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
        /// Deserialized only: the tests assert on the registration's diagnostics.
        #[allow(dead_code)]
        value: Option<u32>,
    }

    /// A plugin whose construction cannot fail, so only the settings decide the outcome.
    struct TestPlugin;

    impl Plugin for TestPlugin {
        const NAME: &'static str = "test_plugin";
        type Settings = TestSettings;

        fn new(_: &Context, _: &Self::Settings, _: &mut Subscriptions) -> Result<Self, Error> {
            Ok(Self)
        }
    }

    fn registration() -> PluginRegistration {
        PluginRegistration::new::<TestPlugin>()
    }

    /// Builds the `[plugins.<name>]` section value the host hands to the registration.
    fn section(entries: &[(&str, Value)]) -> Value {
        let mut dict = Dict::new();
        for (key, value) in entries {
            dict.insert((*key).to_owned(), value.clone());
        }

        Value::from(dict)
    }

    #[test]
    fn omitted_section_has_no_unknown_keys() {
        let unknown = registration()
            .validate_settings(None)
            .expect("the defaults should validate");

        assert_empty!(unknown);
    }

    #[test]
    fn recognized_keys_have_no_unknown_keys() {
        let unknown = registration()
            .validate_settings(Some(&section(&[("value", Value::from(42_u32))])))
            .expect("recognized settings should validate");

        assert_empty!(unknown);
    }

    #[test]
    fn typo_is_reported_as_an_unknown_key() {
        let unknown = registration()
            .validate_settings(Some(&section(&[("valuee", Value::from(42_u32))])))
            .expect("unknown keys should not fail validation");

        assert_eq!(unknown, ["valuee"]);
    }

    #[test]
    fn invalid_value_type_names_the_plugin() {
        let error = registration()
            .validate_settings(Some(&section(&[("value", Value::from("not a number"))])))
            .expect_err("a string is not a number");

        let message = error.to_string();
        assert!(message.contains("invalid settings"), "{message}");
        assert!(message.contains(TestPlugin::NAME), "{message}");
    }
}
