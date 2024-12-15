use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use tracing::{debug, trace};

use crate::Error;

/// The name of a plugin.
pub struct Name(&'static str);
/// The author of a plugin.
pub struct Author(&'static str);
/// The version of a plugin.
pub struct Version(&'static str);

pub struct GoogleSearch {}

#[async_trait]
impl Plugin for GoogleSearch {
    fn new() -> GoogleSearch {
        GoogleSearch {}
    }

    fn name() -> Name {
        Name("google_search")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref message) = message.command {
            if let Some(query) = message.strip_prefix(".g ") {
                debug!("user requested google search");

                client.send_privmsg(channel, format!("searching for {query}"))?;
            }
        }

        Ok(())
    }
}

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
        registry.register::<GoogleSearch>();

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
