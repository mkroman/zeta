#![allow(clippy::doc_markdown)]

use std::sync::Arc;

use irc::client::Client;
use irc::proto::Message;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use url::Url;
use zeta_plugin::Prefix;

pub use crate::context::{Context, SharedState};

pub use zeta_plugin::{Author, Error, Metadata, Name, Plugin};

/// Common includes used in plugins.
#[allow(unused)]
mod prelude {
    pub use async_trait::async_trait;
    pub use irc::client::Client;
    pub use irc::proto::{Command, Message};
    pub use zeta_plugin::Error as ZetaError;
    pub use zeta_plugin::prelude::{ArgsError, BoxError, Prefix, plugin_err, require_env};

    pub use super::{
        Author, Context, Metadata, Name, Plugin, PluginCatalog, PluginInfo, SharedState,
    };
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
            fn register_bundled_plugins(&mut self, #[allow(unused)] ctx: &Context) {
                $(
                    #[cfg(feature = $feature)]
                    {
                        // Explicitly uses the module and struct passed in
                        self.register::<$mod_name::$struct_name>(ctx);
                    }
                )*
            }
        }
    }
}

declare_plugins! {
  /// Time-based user alerts.
  #[cfg(feature = "plugin-alert")]
  alert::AlertPlugin,

  /// Chaturbate platform integration.
  #[cfg(feature = "plugin-chaturbate")]
  chaturbate::Chaturbate,

  /// Plugin that helps the user make a choice
  #[cfg(feature = "plugin-choices")]
  choices::Choices,

  /// Crypto currency quotes via CoinMarketCap
  #[cfg(feature = "plugin-cryptocoins")]
  cryptocoins::CryptoCoins,

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

  /// List loaded plugins and their commands
  #[cfg(feature = "plugin-help")]
  help::Help,

  /// Howlongtobeat.com integration
  #[cfg(feature = "plugin-howlongtobeat")]
  howlongtobeat::HowLongToBeat,

  /// IMDb integration
  #[cfg(feature = "plugin-imdb")]
  imdb::Imdb,

  /// Is it open
  #[cfg(feature = "plugin-isitopen")]
  isitopen::IsItOpen,

  /// Kagi search integration
  #[cfg(feature = "plugin-kagi")]
  kagi::KagiPlugin,

  /// User notifications
  #[cfg(feature = "plugin-notification")]
  notification::NotificationPlugin,

  /// URL history
  #[cfg(feature = "plugin-ofn")]
  ofn::Ofn,

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

/// Metadata about a registered plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginInfo {
    /// The name of the plugin.
    pub name: String,
    /// The authors of the plugin.
    pub authors: Vec<Author>,
    /// The prefix commands handled by the plugin.
    pub commands: &'static [Prefix],
}

/// Snapshot of the plugins registered with the bot and the commands they handle.
///
/// The catalog is published to [`Context::shared`] when the registry is preloaded, so plugins can
/// look it up with [`SharedState::get`] to discover other plugins.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PluginCatalog {
    /// The registered plugins, in registration order.
    pub plugins: Vec<PluginInfo>,
}

/// Plugin registry.
#[derive(Default)]
pub struct Registry {
    /// List of loaded plugins (name, plugin).
    pub plugins: Vec<(String, Box<dyn Plugin<Context>>)>,
    /// Catalog of the registered plugins, published to [`Context::shared`].
    catalog: PluginCatalog,
    /// List of plugins that failed to initialize.
    pub failed: Vec<(String, Error)>,
}

impl Registry {
    /// Constructs and returns a new, empty plugin registry.
    #[must_use]
    pub fn new() -> Registry {
        Registry {
            plugins: vec![],
            catalog: PluginCatalog::default(),
            failed: vec![],
        }
    }

    /// Constructs and returns a new plugin registry with initialized plugins.
    pub fn preloaded(ctx: &Context) -> Registry {
        let mut registry = Self::new();
        debug!("registering plugins");

        registry.register_bundled_plugins(ctx);

        let num_plugins = registry.plugins.len();
        let num_failed = registry.failed.len();
        debug!(%num_plugins, %num_failed, "finished registering plugins");

        if num_failed > 0 {
            warn!(%num_failed, "some plugins failed to initialize");
        }

        ctx.shared.publish(Arc::new(registry.catalog.clone()));

        registry
    }

    /// Registers a new plugin based on its type.
    ///
    /// Returns `true` if the plugin was successfully initialized and registered, `false` if
    /// initialization failed. Failed plugins are tracked in `self.failed` and logged with their
    /// name and error.
    pub fn register<P: Plugin<Context> + 'static>(&mut self, ctx: &Context) -> bool {
        let metadata = P::metadata();
        let name = metadata.name.to_string();

        match P::new(ctx) {
            Ok(plugin) => {
                debug!(plugin = %name, "registered plugin");

                self.catalog.plugins.push(PluginInfo {
                    name: name.clone(),
                    authors: metadata.authors,
                    commands: plugin.commands(),
                });
                self.plugins.push((name, Box::new(plugin)));

                true
            }
            Err(e) => {
                warn!(plugin = %name, error = %e, "failed to initialize plugin");
                self.failed.push((name, e));
                false
            }
        }
    }

    /// Removes and returns all loaded plugins, leaving the registry empty.
    ///
    /// The caller is expected to move each plugin into its own task.
    #[must_use]
    pub fn take_plugins(&mut self) -> Vec<(String, Box<dyn Plugin<Context>>)> {
        std::mem::take(&mut self.plugins)
    }
}

/// A plugin running in its own long-lived task.
///
/// The task loads the plugin and then waits for IRC messages on an unbounded channel, handling
/// them one at a time. State that other plugins should be able to access must be published to
/// [`Context::shared`] when the plugin is constructed or loaded.
pub struct PluginTask {
    /// The name of the plugin, used for logging.
    pub name: String,
    /// The mailbox of the plugin task.
    sender: mpsc::UnboundedSender<Arc<Message>>,
}

impl PluginTask {
    /// Spawns `plugin` into a task that loads it and processes messages until the channel closes.
    ///
    /// If the plugin fails to load, the error is logged and the task exits.
    pub fn spawn(
        name: String,
        mut plugin: Box<dyn Plugin<Context>>,
        ctx: Arc<Context>,
        client: Arc<Client>,
    ) -> Self {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Arc<Message>>();
        let task_name = name.clone();

        tokio::spawn(async move {
            debug!(plugin = %task_name, "plugin task started");

            if let Err(error) = plugin.loaded(&ctx, &client).await {
                warn!(plugin = %task_name, %error, "plugin failed to load");

                return;
            }

            while let Some(message) = receiver.recv().await {
                if let Err(error) = plugin.handle_message(&ctx, &client, &message).await {
                    warn!(plugin = %task_name, %error, "plugin error during message handling");
                }
            }

            debug!(plugin = %task_name, "plugin task stopped");
        });

        Self { name, sender }
    }

    /// Queues `message` for the plugin task.
    ///
    /// # Errors
    ///
    /// Returns an error if the plugin task has stopped.
    pub fn send(&self, message: Arc<Message>) -> Result<(), mpsc::error::SendError<Arc<Message>>> {
        self.sender.send(message)
    }
}

/// Extracts HTTP(s) URLs from a string.
#[must_use]
#[allow(unused)]
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
