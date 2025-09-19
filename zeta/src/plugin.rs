use async_trait::async_trait;
use irc::client::Client;
use irc::proto::Message;
use reqwest::redirect::Policy;
use tracing::debug;
use url::Url;

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

/// Calculator plugin based on rink
pub mod calculator;
/// Plugin that helps the user make a choice
pub mod choices;
/// Query nameservers
pub mod dig;
/// Query geolocation of addresses and hostnames
pub mod geoip;
/// Search google
pub mod google_search;
/// Process health information
pub mod health;
/// Reddit plugin integration
pub mod reddit;
/// Generic string utilliy plugin
pub mod string_utils;
/// TikTok integration
pub mod tiktok;
/// TVmaze integration
pub mod tvmaze;
/// Urban Dictionary integration
pub mod urban_dictionary;
/// YouTube integration
pub mod youtube;

/// Common includes used in plugins.
#[allow(unused)]
mod prelude {
    pub use super::{Author, Name, Plugin, Version};
    pub use crate::Error as ZetaError;
    pub use async_trait::async_trait;
    pub use irc::client::Client;
    pub use irc::proto::{Command, Message};
}

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

    /// Process an IRC protocol message.
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

        registry.register::<calculator::Calculator>();
        registry.register::<choices::Choices>();
        registry.register::<dig::Dig>();
        registry.register::<geoip::GeoIp>();
        registry.register::<google_search::GoogleSearch>();
        registry.register::<health::Health>();
        registry.register::<reddit::Reddit>();
        registry.register::<string_utils::StringUtils>();
        registry.register::<tiktok::Tiktok>();
        registry.register::<tvmaze::Tvmaze>();
        registry.register::<urban_dictionary::UrbanDictionary>();
        registry.register::<youtube::YouTube>();

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
///
/// # Panics
///
/// Panics if the default HTTP client fails to build.
#[must_use]
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

/// Extracts HTTP(s) URLs from a string.
#[must_use]
pub fn extract_urls(s: &str) -> Option<Vec<Url>> {
    let urls: Vec<Url> = s
        .split(' ')
        .filter(|word| word.to_ascii_lowercase().starts_with("http"))
        .filter_map(|word| Url::parse(word).ok())
        .collect();

    (!urls.is_empty()).then_some(urls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_extraction() {
        let tests = [
            ("hello https://example.com world", 1),
            ("ftp://example.com/some/file.zip", 0),
            ("http://example.com/some/file.html", 1),
        ];

        for (input, expected_results) in tests {
            let num_urls = extract_urls(input).iter().len();

            assert_eq!(num_urls, expected_results);
        }
    }
}
