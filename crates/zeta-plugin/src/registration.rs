//! Self-registration of plugins through the [`inventory`] crate.
//!
//! Plugins written with the `#[plugin]` attribute macro submit a [`PluginRegistration`] while
//! the crate is linked, so the host can discover every compiled-in plugin without a central
//! list. The registration carries the plugin's name and a monomorphized constructor thunk that
//! deserializes the plugin's `[plugins.<name>]` settings and builds the plugin on demand.

use figment::value::Value;
use serde::de::DeserializeOwned;

use crate::Error;
use crate::context::Context;
use crate::error::plugin_err;
use crate::event::Subscriptions;
use crate::plugin::Plugin;
use crate::registry::ErasedPlugin;

inventory::collect!(PluginRegistration);

/// A plugin made available to the host for registration.
///
/// The `#[plugin]` attribute macro emits an [`inventory::submit!`] for one of these next to the
/// generated [`Plugin`](crate::Plugin) implementation; the host collects them with
/// [`inventory::iter`] and hands each one to
/// [`Registry::register_with`](crate::Registry::register_with).
pub struct PluginRegistration {
    /// The plugin's name (`Plugin::NAME`), used as its `[plugins.<name>]` section and in the
    /// plugin catalog.
    pub name: &'static str,
    /// Deserializes the plugin's settings section, if any, and constructs the plugin.
    ///
    /// Called by [`Registry::register_with`](crate::Registry::register_with); plugins never call
    /// it themselves.
    #[doc(hidden)]
    pub construct: fn(&Context, Option<Value>) -> Result<ConstructedPlugin, Error>,
}

impl PluginRegistration {
    /// Constructs the registration for the plugin type `P`.
    ///
    /// The `#[plugin]` attribute macro calls this in the [`inventory::submit!`] it generates;
    /// plugins never call it themselves. The thunk this builds returns an error when the
    /// plugin's settings section does not deserialize into `P::Settings`, or when
    /// [`Plugin::new`](crate::Plugin::new) fails.
    #[must_use]
    pub const fn new<P>(name: &'static str) -> Self
    where
        P: Plugin<Context> + 'static,
        P::Settings: Default + DeserializeOwned,
    {
        Self {
            name,
            construct: construct::<P>,
        }
    }
}

/// A plugin constructed from its registration, with the subscriptions and configuration
/// diagnostics the registry needs before committing it.
pub struct ConstructedPlugin {
    /// The constructed plugin, with its settings type erased.
    pub plugin: Box<dyn ErasedPlugin>,
    /// The events the plugin registered interest in during construction.
    pub subscriptions: Subscriptions,
    /// Keys in the plugin's configuration section that its settings type did not recognize.
    pub unknown_keys: Vec<String>,
}

/// The constructor thunk [`PluginRegistration::new`] stores: deserialize the settings section,
/// build the plugin, and return it with its declared subscriptions.
fn construct<P>(ctx: &Context, section: Option<Value>) -> Result<ConstructedPlugin, Error>
where
    P: Plugin<Context> + 'static,
    P::Settings: Default + DeserializeOwned,
{
    let (settings, unknown_keys) = match section {
        Some(value) => {
            let mut unknown_keys = Vec::new();
            let settings = serde_ignored::deserialize(&value, |path| {
                unknown_keys.push(path.to_string());
            })
            .map_err(|error| {
                plugin_err(std::io::Error::other(format!(
                    "invalid settings for plugin `{}`: {error}",
                    P::NAME
                )))
            })?;

            (settings, unknown_keys)
        }
        // No section at all: the plugin runs with its defaults. `enabled_by_default` has
        // already decided whether it is enabled.
        None => (P::Settings::default(), Vec::new()),
    };

    let mut subscriptions = Subscriptions::new();
    let plugin = P::new(ctx, &settings, &mut subscriptions)?;

    Ok(ConstructedPlugin {
        plugin: Box::new(plugin),
        subscriptions,
        unknown_keys,
    })
}
