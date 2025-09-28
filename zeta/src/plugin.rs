use async_trait::async_trait;
use irc::client::Client;
use irc::proto::Message;
use tracing::debug;
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

/// Calculator plugin based on rink
#[cfg(feature = "plugin-calculator")]
pub mod calculator;
/// Plugin that helps the user make a choice
#[cfg(feature = "plugin-choices")]
pub mod choices;
/// Query the danish dictionary
#[cfg(feature = "plugin-dendanskeordbog")]
pub mod dendanskeordbog;
/// Query nameservers
#[cfg(feature = "plugin-dig")]
pub mod dig;
/// Query geolocation of addresses and hostnames
#[cfg(feature = "plugin-geoip")]
pub mod geoip;
/// Search google
#[cfg(feature = "plugin-google-search")]
pub mod google_search;
/// Process health information
#[cfg(feature = "plugin-health")]
pub mod health;
/// Reddit plugin integration
#[cfg(feature = "plugin-reddit")]
pub mod reddit;
/// Generic string utilliy plugin
#[cfg(feature = "plugin-string-utils")]
pub mod string_utils;
/// TikTok integration
#[cfg(feature = "plugin-tiktok")]
pub mod tiktok;
#[cfg(feature = "plugin-tvmaze")]
pub mod tvmaze;
/// Urban Dictionary integration
#[cfg(feature = "plugin-urban-dictionary")]
pub mod urban_dictionary;
/// YouTube integration
#[cfg(feature = "plugin-youtube")]
pub mod youtube;

/// Common includes used in plugins.
#[allow(unused)]
mod prelude {
    pub use super::{Author, Name, Plugin, Version};
    pub use crate::Error as ZetaError;
    pub use crate::command::Command as ZetaCommand;
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

        #[cfg(feature = "plugin-calculator")]
        registry.register::<calculator::Calculator>();
        #[cfg(feature = "plugin-choices")]
        registry.register::<choices::Choices>();
        #[cfg(feature = "plugin-dendanskeordbog")]
        registry.register::<dendanskeordbog::DenDanskeOrdbog>();
        #[cfg(feature = "plugin-dig")]
        registry.register::<dig::Dig>();
        #[cfg(feature = "plugin-geoip")]
        registry.register::<geoip::GeoIp>();
        #[cfg(feature = "plugin-google-search")]
        registry.register::<google_search::GoogleSearch>();
        #[cfg(feature = "plugin-health")]
        registry.register::<health::Health>();
        #[cfg(feature = "plugin-reddit")]
        registry.register::<reddit::Reddit>();
        #[cfg(feature = "plugin-string-utils")]
        registry.register::<string_utils::StringUtils>();
        #[cfg(feature = "plugin-tiktok")]
        registry.register::<tiktok::Tiktok>();
        #[cfg(feature = "plugin-tvmaze")]
        registry.register::<tvmaze::Tvmaze>();
        #[cfg(feature = "plugin-urban-dictionary")]
        registry.register::<urban_dictionary::UrbanDictionary>();
        #[cfg(feature = "plugin-youtube")]
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
