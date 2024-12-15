use std::time::Duration;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use reqwest::header;
use tokio::runtime::Handle;
use tokio::time::Instant;
use tracing::{debug, trace};
use url::Url;

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

pub struct Health;

pub struct GoogleSearch {
    client: reqwest::Client,
}

impl GoogleSearch {
    async fn search(&self, _query: &str) -> Result<(), Error> {
        let url = Url::parse("https://www.google.dk/search?hl=en").unwrap();
        let res = self.client.get(url).send().await.unwrap();

        debug!(?res, "performed google search");

        Ok(())
    }
}

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0";

#[async_trait]
impl Plugin for GoogleSearch {
    fn new() -> GoogleSearch {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "Accept-Language",
            header::HeaderValue::from_static("en-US,en;q=0.5"),
        );
        headers.insert("Accept-Charset", header::HeaderValue::from_static("utf-8"));

        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        GoogleSearch { client }
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

                let now = Instant::now();

                if let Ok(_res) = self.search(query).await {
                    client.send_privmsg(
                        channel,
                        format!("got results in {}ms", now.elapsed().as_millis()),
                    )?;
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Plugin for Health {
    fn new() -> Health {
        Health
    }

    fn name() -> Name {
        Name("health")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref message) = message.command {
            if message.starts_with(".health") {
                let metrics = Handle::current().metrics();

                let num_workers = metrics.num_workers();
                let num_alive_tasks = metrics.num_alive_tasks();

                client.send_privmsg(
                    channel,
                    format!(
                        "\x0310>\x0f\x02 Health:\x02\x0310 Worker threads:\x0f {num_workers}\x0310 Alive tasks:\x0f {num_alive_tasks}",
                    ),
                )?;
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
        registry.register::<Health>();

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
