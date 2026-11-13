//! Plugin registry.
//!
//! The registry constructs plugins, keeps the loaded ones with the subscriptions they declared,
//! and tracks the ones that failed to initialize. The facade's generated [`PluginSet`] maps a
//! configuration section's plugin name to the compiled-in plugin (see `crates/plugins`), so the
//! registry has no runtime registration mechanism and no `dyn Plugin`. The host then spawns each
//! [`LoadedPlugin`] into its own long-lived task and routes events to them through the dispatch
//! index.

use std::sync::Arc;

use figment::value::Value;
use tracing::{debug, error, warn};

use zeta_plugin::{CatalogEntry, Context, Error, LoadedPlugin, PluginCatalog, PluginSet};

/// Plugin registry.
pub struct Registry<P> {
    /// The loaded plugins, in registration order.
    plugins: Vec<LoadedPlugin<P>>,
    /// Plugins that failed to initialize, with the error that stopped them.
    failed_plugins: Vec<(&'static str, Error)>,
}

impl<P> Default for Registry<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P> Registry<P> {
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
    pub fn take_plugins(&mut self) -> Vec<LoadedPlugin<P>> {
        std::mem::take(&mut self.plugins)
    }
}

impl<P> Registry<P>
where
    P: PluginSet,
{
    /// Constructs and registers the plugin named `name` from its configuration section.
    ///
    /// `section` is the plugin's raw `[plugins.<name>]` configuration section, with the
    /// host-managed `enabled` key removed; `None` when the section is omitted, in which case the
    /// plugin's settings take their defaults. [`PluginSet::construct`] deserializes the section
    /// into the plugin's own settings type and constructs the plugin; unrecognized settings keys
    /// are reported once when the configuration is loaded.
    ///
    /// Failed plugins are recorded and logged with their name and error, and their names are
    /// reported by [`Registry::failed_plugin_names`].
    pub fn register(&mut self, ctx: &Context, name: &'static str, section: Option<&Value>) {
        match P::construct(ctx, name, section) {
            Ok(Some(loaded)) => {
                debug!(zeta.plugin.name = %loaded.name, "registered plugin");
                self.plugins.push(loaded);
            }
            // The facade only asks for its own compiled-in names, so an unknown name is a
            // programming error rather than a configuration one.
            Ok(None) => error!(zeta.plugin.name = %name, "no compiled-in plugin has that name"),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_plugin::{CommandSpec, Plugin, Registered};
    use zeta_test_support::{RecordingPlugin, publish_recording, test_context};

    /// A one-plugin set mirroring the facade's generated enum.
    enum TestPlugins {
        /// The recording plugin.
        Recording(RecordingPlugin),
    }

    impl Plugin for TestPlugins {
        async fn handle<'a>(
            &'a self,
            ctx: &'a Context,
            client: &'a zeta_plugin::Client,
            event: &'a zeta_plugin::Event,
        ) -> Result<(), Error> {
            match self {
                Self::Recording(plugin) => plugin.handle(ctx, client, event).await,
            }
        }

        async fn shutdown<'a>(
            &'a mut self,
            ctx: &'a Context,
            client: &'a zeta_plugin::Client,
        ) -> Result<(), Error> {
            match self {
                Self::Recording(plugin) => plugin.shutdown(ctx, client).await,
            }
        }
    }

    impl PluginSet for TestPlugins {
        fn construct(
            ctx: &Context,
            name: &str,
            section: Option<&Value>,
        ) -> Result<Option<LoadedPlugin<Self>>, Error> {
            if name == RecordingPlugin::NAME {
                let loaded = LoadedPlugin::<RecordingPlugin>::from_section(ctx, section)?;

                return Ok(Some(loaded.map(Self::Recording)));
            }

            Ok(None)
        }

        fn validate(name: &str, section: Option<&Value>) -> Option<Result<Vec<String>, Error>> {
            (name == RecordingPlugin::NAME)
                .then(|| zeta_plugin::validate_settings::<RecordingPlugin>(section))
        }
    }

    fn loaded() -> LoadedPlugin<TestPlugins> {
        let ctx = test_context();
        publish_recording(&ctx);

        TestPlugins::construct(&ctx, RecordingPlugin::NAME, None)
            .expect("the plugin should construct")
            .expect("the name should be known")
    }

    #[tokio::test]
    async fn register_constructs_the_named_plugin() {
        let ctx = test_context();
        publish_recording(&ctx);
        let mut registry = Registry::<TestPlugins>::new();

        registry.register(&ctx, RecordingPlugin::NAME, None);

        assert_eq!(registry.len(), 1);
        assert!(registry.failed_plugin_names().next().is_none());

        let plugins = registry.take_plugins();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, RecordingPlugin::NAME);
    }

    #[tokio::test]
    async fn unknown_plugin_names_are_not_registered() {
        let ctx = test_context();
        let mut registry = Registry::<TestPlugins>::new();

        registry.register(&ctx, "not_a_plugin", None);

        assert!(registry.is_empty());
        assert!(registry.failed_plugin_names().next().is_none());
    }

    #[tokio::test]
    async fn publish_catalog_derives_entries_from_the_loaded_plugins() {
        let ctx = test_context();
        publish_recording(&ctx);
        let mut registry = Registry::<TestPlugins>::new();

        registry.register(&ctx, RecordingPlugin::NAME, None);
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

    /// The `LoadedPlugin` envelope carries the name and subscriptions the runtime routes with.
    #[tokio::test]
    async fn loaded_plugin_carries_its_subscriptions() {
        let loaded = loaded();

        assert_eq!(loaded.name, RecordingPlugin::NAME);
        assert_eq!(
            loaded.subscriptions.commands(),
            &[CommandSpec::new(".recording", "record a message")]
        );
    }
}
