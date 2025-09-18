use async_trait::async_trait;
use irc::client::Client;
use irc::proto::Message;
use reqwest::redirect::Policy;
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
pub mod reddit;
pub mod string_utils;
pub mod tiktok;
pub mod tvmaze;
pub mod urban_dictionary;
pub mod youtube;

/// The base trait that all plugins must implement.
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

/// Plugin registry.
#[derive(Default)]
pub struct Registry {
    /// List of loaded plugins.
    pub plugins: Vec<Box<dyn Plugin>>,
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    #[must_use]
    pub fn new() -> Registry {
        Registry { plugins: vec![] }
    }

    /// Constructs and returns a new plugin registry with initialized plugins.
    pub fn preloaded() -> Registry {
        let mut registry = Self::new();
        debug!("registering plugins");

        registry.register::<health::Health>();
        registry.register::<dig::Dig>();
        registry.register::<choices::Choices>();
        registry.register::<google_search::GoogleSearch>();
        registry.register::<calculator::Calculator>();
        registry.register::<geoip::GeoIp>();
        registry.register::<youtube::YouTube>();
        registry.register::<tvmaze::Tvmaze>();
        registry.register::<reddit::Reddit>();
        registry.register::<string_utils::StringUtils>();
        registry.register::<urban_dictionary::UrbanDictionary>();
        registry.register::<tiktok::Tiktok>();

        let num_plugins = registry.plugins.len();
        debug!(%num_plugins, "finished registering plugins");

        registry
    }

    /// Registers a new plugin based on its type.
    pub fn register<P: Plugin + 'static>(&mut self) -> bool {
        let plugin = Box::new(P::new());

        self.plugins.push(plugin);

        true
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
        .redirect(Policy::none())
        .timeout(consts::HTTP_TIMEOUT)
        .user_agent(consts::HTTP_USER_AGENT)
}
