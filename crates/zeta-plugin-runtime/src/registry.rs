//! Plugin registry.
//!
//! The registry constructs plugins, keeps the loaded ones with the subscriptions they declared,
//! and tracks the ones that failed to initialize. The host discovers compiled-in plugins through
//! `inventory` and registers them with [`Registry::register_with`] (the `plugins` facade drives
//! this). The host then spawns each [`LoadedPlugin`] into its own long-lived task and routes
//! events to them through the dispatch index.

use std::sync::Arc;

use figment::value::Value;
use tracing::{debug, error, warn};

use zeta_plugin::{CatalogEntry, Context, Error, LoadedPlugin, PluginCatalog, PluginRegistration};

/// Plugin registry.
pub struct Registry {
    /// The loaded plugins, in registration order.
    plugins: Vec<LoadedPlugin>,
    /// Plugins that failed to initialize, with the error that stopped them.
    failed_plugins: Vec<(&'static str, Error)>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            plugins: vec![],
            failed_plugins: vec![],
        }
    }

    /// Publishes the catalog of registered plugins to the context's shared state.
    ///
    /// Called once after all plugins have been registered, so plugins can discover each other
    /// through the catalog. The catalog is derived from the registered plugins' subscriptions,
    /// so it cannot drift from them.
    ///
    /// Plugins may cache the handle to keep shared-state lookups out of their hot paths, so a
    /// catalog published later would not reach every plugin.
    pub fn publish_catalog(&self, ctx: &Context) {
        let entries = self
            .plugins
            .iter()
            .map(|plugin| {
                CatalogEntry::new(
                    plugin.name.to_string(),
                    plugin.subscriptions.commands().to_vec(),
                    plugin.subscriptions.url_scope(),
                )
            })
            .collect();

        ctx.shared.publish(Arc::new(PluginCatalog::new(entries)));
    }

    /// Registers a plugin from its [`PluginRegistration`].
    ///
    /// `section` is the plugin's raw `[plugins.<name>]` configuration section, with the
    /// host-managed `enabled` key removed; `None` when the section is omitted, in which case the
    /// plugin's settings take their defaults. The registration's thunk deserializes the section
    /// into the plugin's own settings type and constructs the plugin; unrecognized settings keys
    /// are reported once when the configuration is loaded.
    ///
    /// Failed plugins are recorded and logged with their name and error, and their names are
    /// reported by [`Registry::failed_plugin_names`]. A plugin whose name is already registered
    /// or has already failed is not registered again.
    pub fn register_with(
        &mut self,
        registration: &PluginRegistration,
        ctx: &Context,
        section: Option<&Value>,
    ) {
        let name = registration.name;
        let already_failed = self
            .failed_plugins
            .iter()
            .any(|(failed, _)| *failed == name);

        if already_failed || self.plugins.iter().any(|plugin| plugin.name == name) {
            let error = Error::Message(format!("duplicate plugin registration for `{name}`"));
            error!(
                zeta.plugin.name = %name,
                exception.message = %error,
                "duplicate plugin registration"
            );

            if !already_failed {
                self.failed_plugins.push((name, error));
            }

            return;
        }

        match (registration.construct)(ctx, section) {
            Ok(loaded) => {
                debug!(zeta.plugin.name = %loaded.name, "registered plugin");
                self.plugins.push(loaded);
            }
            Err(error) => self.fail(name, error),
        }
    }

    /// Records a plugin initialization failure.
    fn fail(&mut self, name: &'static str, error: Error) {
        warn!(
            zeta.plugin.name = %name,
            exception.message = %error,
            "failed to initialize plugin"
        );
        self.failed_plugins.push((name, error));
    }

    /// Returns the number of plugins that were successfully initialized.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Returns whether no plugins were successfully initialized.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Returns the names of the plugins that failed to initialize, in registration order.
    ///
    /// The errors themselves are logged when they happen; the host uses this to report one
    /// startup summary instead of leaving the failures scattered across the log.
    pub fn failed_plugin_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.failed_plugins.iter().map(|(name, _)| *name)
    }

    /// Removes and returns all loaded plugins, leaving the registry empty.
    ///
    /// The caller is expected to move each plugin into its own task.
    #[must_use]
    pub fn take_plugins(&mut self) -> Vec<LoadedPlugin> {
        std::mem::take(&mut self.plugins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_plugin::{CommandSpec, Registered};
    use zeta_test_support::{RecordingPlugin, publish_recording, test_context};

    #[tokio::test]
    async fn duplicate_registrations_are_rejected() {
        let ctx = test_context();
        publish_recording(&ctx);
        let mut registry = Registry::new();
        let registration = PluginRegistration::new::<RecordingPlugin>();

        registry.register_with(&registration, &ctx, None);
        // A name can only be registered once; the duplicate lands in the failure list.
        registry.register_with(&registration, &ctx, None);

        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.failed_plugin_names().collect::<Vec<_>>(),
            [RecordingPlugin::NAME]
        );
    }

    #[tokio::test]
    async fn publish_catalog_derives_entries_from_the_loaded_plugins() {
        let ctx = test_context();
        publish_recording(&ctx);
        let mut registry = Registry::new();

        registry.register_with(&PluginRegistration::new::<RecordingPlugin>(), &ctx, None);

        registry.publish_catalog(&ctx);

        let catalog = ctx
            .shared
            .get::<PluginCatalog>()
            .expect("the catalog should be published");

        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.entries[0].name, RecordingPlugin::NAME);
        assert_eq!(
            catalog.entries[0].commands,
            [CommandSpec::new(".recording", "record a message")]
        );
    }
}
