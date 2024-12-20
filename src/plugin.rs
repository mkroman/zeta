use async_trait::async_trait;
use irc::client::Client;
use irc::proto::Message;
use tracing::trace;

use crate::Error;

/// The name of a plugin.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Name(&'static str);
/// The author of a plugin.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Author(&'static str);
/// The version of a plugin.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Version(&'static str);

pub mod dig;
pub mod health;

#[async_trait]
pub trait Plugin: Send + Sync {
    /// The name of the plugin.
    fn name() -> Name
    where
        Self: Sized;

    /// The author of the plugin.
    fn author() -> Author
    where
        Self: Sized;

    /// The version of the plugin.
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

#[derive(Default)]
pub struct Registry {
    pub plugins: Vec<Box<dyn Plugin>>,
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    pub fn new() -> Registry {
        Registry { plugins: vec![] }
    }

    /// Constructs and returns a new plugin registry with initialized plugins.
    pub fn preloaded() -> Registry {
        let mut registry = Self::new();
        trace!("Registering plugins");
        registry.register::<health::Health>();
        registry.register::<dig::Dig>();

        let num_plugins = registry.plugins.len();
        trace!(%num_plugins, "Done registering plugins");
        registry
    }

    /// Registers a new plugin based on its type.
    pub fn register<P: Plugin + 'static>(&mut self) -> bool {
        let plugin = Box::new(P::new());

        self.plugins.push(plugin);

        true
    }
}
