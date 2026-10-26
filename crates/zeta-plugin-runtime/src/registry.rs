//! Plugin registry.
//!
//! The registry constructs plugins, keeps the loaded ones with the subscriptions they declared,
//! and tracks the ones that failed to initialize. The host discovers compiled-in plugins through
//! `inventory` and registers them with [`Registry::register_with`] (the `plugins` facade drives
//! this). The host then spawns each [`RegisteredPlugin`] into its own
//! [`PluginTask`](crate::PluginTask) and routes events to them through the dispatch index.

use std::sync::Arc;

use figment::value::Value;
use tracing::{debug, error, warn};

use zeta_plugin::prelude::plugin_err;
use zeta_plugin::{
    CatalogEntry, ConstructedPlugin, Context, ErasedPlugin, Error, PluginCatalog,
    PluginRegistration, Subscriptions,
};

/// A plugin registered with the bot, together with the subscriptions it declared.
pub struct RegisteredPlugin {
    /// The name of the plugin.
    pub name: &'static str,
    /// The plugin itself.
    pub plugin: Box<dyn ErasedPlugin>,
    /// The events the plugin registered interest in during initialization.
    pub subscriptions: Subscriptions,
}

/// Plugin registry.
#[derive(Default)]
pub struct Registry {
    /// The loaded plugins, in registration order.
    plugins: Vec<RegisteredPlugin>,
    /// Catalog of the registered plugins, published to the context's shared state.
    catalog: PluginCatalog,
    /// Plugins that failed to initialize, with the error that stopped them.
    failed: Vec<(&'static str, Error)>,
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugins: vec![],
            catalog: PluginCatalog::default(),
            failed: vec![],
        }
    }

    /// Publishes the catalog of registered plugins to the context's shared state.
    ///
    /// Called once after all plugins have been registered, so plugins can discover each other
    /// through the catalog.
    pub fn publish_catalog(&self, ctx: &Context) {
        ctx.shared.publish(Arc::new(self.catalog.clone()));
    }

    /// Registers a plugin from its [`PluginRegistration`].
    ///
    /// `section` is the plugin's raw `[plugins.<name>]` configuration section, with the
    /// host-managed `enabled` key removed; `None` when the section is omitted, in which case the
    /// plugin's settings take their defaults. The registration's thunk deserializes the section
    /// into the plugin's own settings type and constructs the plugin; unrecognized settings keys
    /// are reported once when the configuration is loaded.
    ///
    /// Returns `true` if the plugin was successfully initialized and registered, `false` if it
    /// failed or if another plugin with the same name is already registered (or has failed to
    /// initialize). Failed plugins are tracked in `self.failed` and logged with their name and
    /// error.
    pub fn register_with(
        &mut self,
        registration: &PluginRegistration,
        ctx: &Context,
        section: Option<&Value>,
    ) -> bool {
        let name = registration.name;
        let duplicate = self.plugins.iter().any(|plugin| plugin.name == name)
            || self.failed.iter().any(|(failed, _)| *failed == name);

        if duplicate {
            let error = plugin_err(std::io::Error::other(format!(
                "duplicate plugin registration for `{name}`"
            )));
            error!(plugin = %name, %error, "duplicate plugin registration");

            if !self.failed.iter().any(|(failed, _)| *failed == name) {
                self.failed.push((name, error));
            }

            return false;
        }

        match (registration.construct)(ctx, section) {
            Ok(constructed) => {
                debug_assert_eq!(
                    constructed.name, name,
                    "the registration and the plugin disagree on the name"
                );

                self.commit(constructed);

                true
            }
            Err(error) => self.fail(name, error),
        }
    }

    /// Records a plugin initialization failure and reports it.
    fn fail(&mut self, name: &'static str, error: Error) -> bool {
        warn!(plugin = %name, %error, "failed to initialize plugin");
        self.failed.push((name, error));

        false
    }

    /// Commits a successfully constructed plugin to the catalog and the loaded-plugin list.
    fn commit(&mut self, constructed: ConstructedPlugin) {
        let ConstructedPlugin {
            name,
            plugin,
            subscriptions,
        } = constructed;

        debug!(plugin = %name, "registered plugin");

        self.catalog.entries.push(CatalogEntry {
            name: name.to_string(),
            commands: subscriptions.commands().to_vec(),
            url_scope: subscriptions.url_scope(),
        });
        self.plugins.push(RegisteredPlugin {
            name,
            plugin,
            subscriptions,
        });
    }

    /// Removes and returns all loaded plugins, leaving the registry empty.
    ///
    /// The caller is expected to move each plugin into its own task.
    #[must_use]
    pub fn take_plugins(&mut self) -> Vec<RegisteredPlugin> {
        std::mem::take(&mut self.plugins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_plugin::event::MessageEvent;
    use zeta_plugin::{NoSettings, Plugin};
    use zeta_test_support::test_context;

    /// The lifecycle log shared between a test and its plugin through the context.
    type Recording = Arc<std::sync::Mutex<Vec<&'static str>>>;

    /// Publishes the recording log through the context's shared state.
    fn publish_recording(ctx: &Context) -> Recording {
        let recording: Recording = Arc::new(std::sync::Mutex::new(Vec::new()));

        assert!(ctx.shared.publish(Arc::clone(&recording)).is_none());

        recording
    }

    /// Test plugin recording the lifecycle events it observes.
    struct RecordingPlugin {
        recording: Recording,
    }

    #[async_trait::async_trait]
    impl Plugin for RecordingPlugin {
        const NAME: &'static str = "recording";

        type Settings = NoSettings;

        fn new(ctx: &Context, _: &NoSettings, _: &mut Subscriptions) -> Result<Self, Error> {
            let recording = ctx
                .shared
                .get::<std::sync::Mutex<Vec<&'static str>>>()
                .expect("recording log should be published");

            Ok(Self { recording })
        }

        async fn handle_message(
            &self,
            _: &Context,
            _: &irc::client::Client,
            _: &MessageEvent,
        ) -> Result<(), Error> {
            zeta_utils::sync::lock(&self.recording).push("message");

            Ok(())
        }
    }

    #[tokio::test]
    async fn duplicate_registrations_are_rejected() {
        let ctx = test_context();
        publish_recording(&ctx);
        let mut registry = Registry::new();
        let registration = PluginRegistration::new::<RecordingPlugin>();

        assert!(registry.register_with(&registration, &ctx, None));
        assert!(
            !registry.register_with(&registration, &ctx, None),
            "a name can only be registered once"
        );

        assert_eq!(registry.plugins.len(), 1);
        assert_eq!(registry.failed.len(), 1);
    }
}
