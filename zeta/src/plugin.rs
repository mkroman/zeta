use tracing::debug;
use url::Url;

pub use zeta_plugin::{Author, Name, Plugin, Version};

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

/// Declares plugin modules and generates a registry helper to avoid boilerplate.
///
/// For each entry, it generates:
/// 1. `pub mod $mod_name;` (with feature gates and docs).
/// 2. A call to `register::<$mod_name::$struct_name>()` inside `Registry::register_bundled_plugins`.
macro_rules! declare_plugins {
    (
        $(
            $(#[doc = $doc:expr])*
            #[cfg(feature = $feature:literal)]
            $mod_name:ident :: $struct_name:ident
        ),* $(,)?
    ) => {
        // Generate module declarations
        $(
            $(#[doc = $doc])*
            #[cfg(feature = $feature)]
            pub mod $mod_name;
        )*

        // Generate a helper extension to register these specific plugins
        impl Registry {
            fn register_bundled_plugins(&mut self) {
                $(
                    #[cfg(feature = $feature)]
                    {
                        // Explicitly uses the module and struct passed in
                        self.register::<$mod_name::$struct_name>();
                    }
                )*
            }
        }
    }
}

declare_plugins! {
    /// Plugin that helps the user make a choice
    #[cfg(feature = "plugin-choices")]
    choices::Choices,

    /// Query the danish dictionary
    #[cfg(feature = "plugin-dendanskeordbog")]
    dendanskeordbog::DenDanskeOrdbog,

    /// Query nameservers
    #[cfg(feature = "plugin-dig")]
    dig::Dig,

    /// Query geolocation of addresses and hostnames
    #[cfg(feature = "plugin-geoip")]
    geoip::GeoIp,

    /// GitHub integration
    #[cfg(feature = "plugin-github")]
    github::GitHubPlugin,

    /// Google images integration
    #[cfg(feature = "plugin-google-images")]
    google_images::GoogleImages,

    /// Process health information
    #[cfg(feature = "plugin-health")]
    health::Health,

    /// Howlongtobeat.com integration
    #[cfg(feature = "plugin-howlongtobeat")]
    howlongtobeat::HowLongToBeat,

    /// Is it open
    #[cfg(feature = "plugin-isitopen")]
    isitopen::IsItOpen,

    /// Kagi search integration
    #[cfg(feature = "plugin-kagi")]
    kagi::KagiPlugin,

    /// Weather service integration
    #[cfg(feature = "plugin-openweathermap")]
    openweathermap::OpenWeatherMap,

    #[cfg(feature = "plugin-pornhub")]
    pornhub::PornHub,

    /// Reddit plugin integration
    #[cfg(feature = "plugin-reddit")]
    reddit::Reddit,

    /// Calculator plugin based on rink
    #[cfg(feature = "plugin-rink")]
    rink::Rink,

    /// Rust Playground integration
    #[cfg(feature = "plugin-rust-playground")]
    rust_playground::RustPlayground,

    /// Spotify integration
    #[cfg(feature = "plugin-spotify")]
    spotify::Spotify,

    /// Generic string utility plugin
    #[cfg(feature = "plugin-string-utils")]
    string_utils::StringUtils,

    /// Thingiverse integration
    #[cfg(feature = "plugin-thingiverse")]
    thingiverse::Thingiverse,

    /// TikTok integration
    #[cfg(feature = "plugin-tiktok")]
    tiktok::Tiktok,

    /// Trustpilot integration
    #[cfg(feature = "plugin-trustpilot")]
    trustpilot::Trustpilot,

    #[cfg(feature = "plugin-tvmaze")]
    tvmaze::Tvmaze,

    // Twitch integration
    #[cfg(feature = "plugin-twitch")]
    twitch::Twitch,

    /// Urban Dictionary integration
    #[cfg(feature = "plugin-urban-dictionary")]
    urban_dictionary::UrbanDictionary,

    /// YouTube integration
    #[cfg(feature = "plugin-youtube")]
    youtube::YouTube,
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

        registry.register_bundled_plugins();

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
