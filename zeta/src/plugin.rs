use std::{collections::HashMap, fmt::Display};

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::Message;
use reqwest::redirect::Policy;
use serde::de::DeserializeOwned;
use tracing::debug;

use crate::{Error, consts};

/// The name of a plugin.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Name(&'static str);
/// The author of a plugin.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Author(&'static str);
/// The version of a plugin.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Version(&'static str);

pub mod calculator;
pub mod choices;
pub mod dig;
pub mod geoip;
pub mod google_search;
pub mod health;
pub mod youtube;

#[async_trait]
pub trait NewPlugin: Send + Sync {
    /// The name of the plugin.
    const NAME: &'static str;
    /// The author of the plugin.
    const AUTHOR: Author;
    /// The version of the plugin.
    const VERSION: Version;

    type Err: std::error::Error;
    type Config: DeserializeOwned;

    /// The constructor for a new plugin.
    fn with_config(config: &Self::Config) -> Self
    where
        Self: Sized;

    async fn handle_message(&self, _message: &Message, _client: &Client) -> Result<(), Error> {
        Ok(())
    }
}

/// A trait for plugins that can be used as trait objects
#[async_trait]
pub trait DynPlugin: Send + Sync {
    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), Error>;
}

// Implement DynPlugin for all NewPlugin types
#[async_trait]
impl<T: NewPlugin> DynPlugin for T {
    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), Error> {
        NewPlugin::handle_message(self, message, client).await
    }
}

type Plugins = Vec<Box<dyn DynPlugin>>;

/// A trait for creating plugins dynamically from configuration
pub trait PluginFactory: Send + Sync {
    fn name(&self) -> &'static str;
    fn create(&self, config: &figment::value::Value) -> Result<Box<dyn DynPlugin>, Error>;
}

/// A factory for creating plugins of a specific type
pub struct TypedPluginFactory<P: NewPlugin + 'static> {
    _phantom: std::marker::PhantomData<P>,
}

impl<P: NewPlugin + 'static> TypedPluginFactory<P> {
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<P: NewPlugin + 'static> PluginFactory for TypedPluginFactory<P> {
    fn name(&self) -> &'static str {
        P::NAME
    }

    fn create(&self, config_value: &figment::value::Value) -> Result<Box<dyn DynPlugin>, Error> {
        let config: P::Config = config_value.deserialize().map_err(|e| {
            Error::ConfigurationError(format!(
                "Failed to deserialize config for {}: {}",
                P::NAME,
                e
            ))
        })?;

        let plugin = P::with_config(&config);
        Ok(Box::new(plugin))
    }
}

#[derive(Default)]
pub struct Registry {
    pub plugins: Plugins,
    factories: HashMap<String, Box<dyn PluginFactory>>,
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    pub fn new() -> Registry {
        let mut registry = Registry {
            plugins: vec![],
            factories: HashMap::new(),
        };

        // Register all available plugin factories
        registry.register_factory(TypedPluginFactory::<dig::Dig>::new());
        registry.register_factory(TypedPluginFactory::<calculator::Calculator>::new());
        registry.register_factory(TypedPluginFactory::<choices::Choices>::new());
        registry.register_factory(TypedPluginFactory::<geoip::GeoIp>::new());
        registry.register_factory(TypedPluginFactory::<google_search::GoogleSearch>::new());
        registry.register_factory(TypedPluginFactory::<health::Health>::new());
        registry.register_factory(TypedPluginFactory::<youtube::YouTube>::new());

        registry
    }

    /// Registers a plugin factory
    pub fn register_factory<F: PluginFactory + 'static>(&mut self, factory: F) {
        let name = factory.name().to_string();
        self.factories.insert(name, Box::new(factory));
    }

    pub fn load_plugins(
        &mut self,
        configs: &HashMap<String, figment::value::Value>,
    ) -> Result<(), Error> {
        self.plugins.clear();

        debug!("registering plugins");

        // Load each plugin based on its configuration
        for (plugin_name, config_value) in configs {
            if let Some(factory) = self.factories.get(plugin_name) {
                match factory.create(config_value) {
                    Ok(plugin) => {
                        debug!(name = plugin_name, "successfully registered plugin");
                        self.plugins.push(plugin);
                    }
                    Err(err) => {
                        debug!(name = plugin_name, error = %err, "failed to register plugin");
                        return Err(err);
                    }
                }
            } else {
                debug!(name = plugin_name, "unknown plugin, skipping");
            }
        }

        Ok(())
    }
}

impl Display for Author {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{}", self.0)
    }
}

impl Display for Version {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{}", self.0)
    }
}

/// Returns a default HTTP client.
pub fn build_http_client() -> reqwest::Client {
    http_client_builder()
        .build()
        .expect("could not build http client")
}

/// Returns a default HTTP client builder.
pub fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::ClientBuilder::new()
        .user_agent(consts::HTTP_USER_AGENT)
        .redirect(Policy::none())
        .timeout(consts::HTTP_TIMEOUT)
}
