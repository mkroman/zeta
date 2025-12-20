use tracing::debug;
use url::Url;

pub use zeta_plugin::{Author, Name, Plugin, Version};

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
/// Howlongtobeat.com integration
#[cfg(feature = "plugin-howlongtobeat")]
pub mod howlongtobeat;
/// Is it open
#[cfg(feature = "plugin-isitopen")]
pub mod isitopen;
#[cfg(feature = "plugin-pornhub")]
pub mod pornhub;
/// Reddit plugin integration
#[cfg(feature = "plugin-reddit")]
pub mod reddit;
/// Calculator plugin based on rink
#[cfg(feature = "plugin-rink")]
pub mod rink;
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
    pub use crate::command::Command as ZetaCommand;
    pub use async_trait::async_trait;
    pub use irc::client::Client;
    pub use irc::proto::{Command, Message};
    pub use irc::proto::{Command as IrcCommand, Message as IrcMessage};
    pub use zeta_plugin::Error as ZetaError;
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

        #[cfg(feature = "plugin-rink")]
        registry.register::<rink::Rink>();
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
        #[cfg(feature = "plugin-howlongtobeat")]
        registry.register::<howlongtobeat::HowLongToBeat>();
        #[cfg(feature = "plugin-isitopen")]
        registry.register::<isitopen::IsItOpen>();
        #[cfg(feature = "plugin-pornhub")]
        registry.register::<pornhub::PornHub>();
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
