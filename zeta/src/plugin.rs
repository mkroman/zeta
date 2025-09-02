use std::{collections::HashMap, fmt::Display};

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::Message;
use reqwest::redirect::Policy;
use serde::de::DeserializeOwned;
use tracing::debug;

use crate::{Error, config::PluginConfig, consts};

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

#[async_trait]
pub trait Plugin: Send + Sync {
    /// Returns the name of the plugin.
    fn name() -> Name
    where
        Self: Sized;

    /// Returns the author of the plugin.
    fn author() -> Author
    where
        Self: Sized;

    /// Returns the version of the plugin.
    fn version() -> Version
    where
        Self: Sized;

    /// The constructor for a new plugin.
    fn new() -> Self
    where
        Self: Sized;

    async fn handle_message(&self, _message: &Message, _client: &Client) -> Result<(), Error> {
        Ok(())
    }
}

type Plugins = Vec<Box<dyn NewPlugin>>;

#[derive(Default)]
pub struct Registry {
    pub plugins: Plugins,
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    pub fn new() -> Registry {
        Registry { plugins: vec![] }
    }

    pub fn load_plugin<P: NewPlugin + 'static, E, C>(&mut self, config: P::Config) {
        debug!(name = P::NAME, "registering plugin");

        let plugin = Box::new(P::with_config(&config));

        self.plugins.push(plugin);
    }

    pub fn load_plugins(
        &mut self,
        configs: &HashMap<String, figment::value::Value>,
    ) -> Result<(), Error> {
        self.plugins.clear();

        debug!("registering plugins");

        Ok(())
    }

    /// Registers a new plugin based on its type.
    pub fn register<P: Plugin + 'static>(&mut self) -> bool {
        let plugin = Box::new(P::new());

        self.plugins.push(plugin);

        true
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
