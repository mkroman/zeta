use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use figment::value::Dict;
use irc::client::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, error, warn};
use zeta_plugin::{CommandSpec, Event, Subscriptions};

pub mod dispatch;

pub use crate::context::{Context, SharedState};

pub use zeta_plugin::{Error, Plugin};

#[allow(unused_imports)]
use zeta_plugin::NoSettings;

/// Common includes used in plugins.
#[allow(unused)]
mod prelude {
    pub use async_trait::async_trait;
    pub use irc::client::Client;
    pub use zeta_plugin::Error as ZetaError;
    pub use zeta_plugin::prelude::{
        ArgsError, BOLD, BoxError, COLOR, CommandEvent, CommandSpec, CtcpEvent, CtcpKind, Event,
        JoinEvent, KickEvent, MessageEvent, NickEvent, NoSettings, PartEvent, QuitEvent,
        REPLY_PREFIX, RESET, RawEvent, Sender, Subscriptions, UrlEvent, UrlScope, notice,
        parse_args_or_usage, parse_words_or_usage, plugin_err, reply, reply_prefix,
        reply_usage_lines, resolve_secret,
    };

    pub use super::{CatalogEntry, Context, Plugin, PluginCatalog, SharedState};

    pub use super::filtering::Filters;
}

/// Generates the two settings tests shared by every plugin with a configuration section.
///
/// Expands to a `default_settings` test asserting the values produced by [`Default`] and a
/// `settings_deserialize` test deserializing the given JSON object before asserting on it.
#[cfg(test)]
macro_rules! settings_tests {
    (
        $ty:ty, $settings:ident,
        default: { $($default:tt)* }
        deserialize: { $($json:tt)* } assert: { $($assert:tt)* }
    ) => {
        #[test]
        fn default_settings() {
            let $settings = <$ty>::default();

            $($default)*
        }

        #[test]
        fn settings_deserialize() {
            let $settings: $ty = serde_json::from_value(serde_json::json!({ $($json)* }))
                .expect("could not deserialize settings");

            $($assert)*
        }
    };
}

/// Declares plugin modules and generates a registry helper to avoid boilerplate.
///
/// For each entry, it generates:
/// 1. `pub mod $mod_name;` (with feature gates and docs).
/// 2. A field in [`PluginsConfig`] holding the plugin's settings type: `mod::Struct => mod::Settings`
///    for plugins with settings, `=> NoSettings` otherwise.
/// 3. A call to `register::<$mod_name::$struct_name>()` inside `Registry::register_bundled_plugins`,
///    gated on the plugin's `enabled` configuration and passing the plugin's own settings through
///    to its constructor.
macro_rules! declare_plugins {
    (
        $(
            $(#[doc = $doc:expr])*
            #[cfg(feature = $feature:literal)]
            $mod_name:ident :: $struct_name:ident => $settings:ty
        ),* $(,)?
    ) => {
        // Generate module declarations. Each plugin module documents itself in its own file;
        // the doc comments in this invocation are only used on the settings fields below.
        $(
            #[cfg(feature = $feature)]
            pub mod $mod_name;
        )*

        // The plugin name is the module name.
        $(
            #[cfg(feature = $feature)]
            impl zeta_plugin::PluginName for $mod_name::$struct_name {
                const NAME: &'static str = stringify!($mod_name);
            }
        )*

        /// Typed, per-plugin configuration extracted from the `[plugins]` section.
        ///
        /// Each plugin has its own section keyed by its module name, e.g. `[plugins.dig]`.
        /// Sections are type-checked at startup and always present; a section that is omitted
        /// entirely defaults to `enabled = true` with default settings. The `enabled` key is
        /// managed by the host; every other key belongs to the plugin's settings type.
        ///
        /// Keys under `[plugins]` that do not match a bundled plugin name are collected in
        /// [`PluginsConfig::unknown`] so the host can warn about likely typos. Unknown keys
        /// inside a section are ignored and reported through
        /// [`PluginConfig::unknown_keys`](crate::config::PluginConfig::unknown_keys), which the
        /// host also warns about.
        #[derive(Clone, Debug, Default, Serialize)]
        pub struct PluginsConfig {
            $(
                $(#[doc = $doc])*
                #[cfg(feature = $feature)]
                pub $mod_name: $crate::config::PluginConfig<$settings>,
            )*

            /// Sections that match no bundled plugin, whether compiled in or not.
            #[serde(flatten)]
            pub unknown: HashMap<String, Dict>,
        }

        // Deserialize the `[plugins]` section one plugin at a time instead of deriving
        // `Deserialize` for the whole struct: a derived implementation deserializes every
        // section in a single generic visitor, which monomorphizes a large visitor over
        // figment's deserializer (one field per plugin) and bloats the binary.
        impl<'de> Deserialize<'de> for PluginsConfig {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                use serde::de::Error as _;

                let mut sections =
                    HashMap::<String, figment::value::Value>::deserialize(deserializer)?;
                let mut config = Self::default();

                $(
                    #[cfg(feature = $feature)]
                    if let Some(section) = sections.remove(stringify!($mod_name)) {
                        config.$mod_name =
                            $crate::config::PluginConfig::<$settings>::deserialize(&section)
                                .map_err(|error| {
                                    D::Error::custom(format!(
                                        "invalid `{}` plugin section: {error}",
                                        stringify!($mod_name)
                                    ))
                                })?;
                    }
                )*

                for (name, section) in sections {
                    let section = Dict::deserialize(&section).map_err(|error| {
                        D::Error::custom(format!("invalid `{name}` plugin section: {error}"))
                    })?;

                    config.unknown.insert(name, section);
                }

                Ok(config)
            }
        }

        /// The names of every bundled plugin, compiled in or not.
        const BUNDLED_PLUGIN_NAMES: &[&str] = &[
            $(
                stringify!($mod_name),
            )*
        ];

        // Generate a helper extension to register these specific plugins
        impl Registry {
            fn register_bundled_plugins(
                &mut self,
                #[allow(unused)] ctx: &Context,
                #[allow(unused)] plugins: &PluginsConfig,
            ) {
                $(
                    #[cfg(feature = $feature)]
                    {
                        let section = &plugins.$mod_name;

                        for key in &section.unknown_keys {
                            ::tracing::warn!(
                                plugin = stringify!($mod_name),
                                key = %key,
                                "unknown settings key (typo, or a setting removed in this version?)"
                            );
                        }

                        if section.enabled {
                            self.register::<$mod_name::$struct_name>(ctx, &section.settings);
                        } else {
                            ::tracing::info!(
                                plugin = stringify!($mod_name),
                                "plugin is disabled by configuration"
                            );
                        }
                    }
                )*
            }
        }
    }
}

/// Filtering support shared with plugins that react to URLs.
///
/// Unlike the plugin modules below, this module is always compiled: it provides the
/// [`Filters`](filtering::Filters) facade that lets URL-handling plugins consult the filter
/// service without depending on the `plugin-filter` feature themselves.
// Without any URL-handling plugin compiled in, nothing consumes the facade.
#[allow(dead_code)]
pub mod filtering;

declare_plugins! {
  /// Channel-scoped URL and sender filters.
  #[cfg(feature = "plugin-filter")]
  filter::FilterPlugin => NoSettings,

  /// Time-based user alerts.
  #[cfg(feature = "plugin-alert")]
  alert::AlertPlugin => alert::Settings,

  /// Chaturbate platform integration.
  #[cfg(feature = "plugin-chaturbate")]
  chaturbate::Chaturbate => NoSettings,

  /// Plugin that helps the user make a choice
  #[cfg(feature = "plugin-choices")]
  choices::Choices => choices::Settings,

  /// Crypto currency quotes via CoinMarketCap
  #[cfg(feature = "plugin-coinmarketcap")]
  coinmarketcap::CoinMarketCap => coinmarketcap::Settings,

  /// Query the danish dictionary
  #[cfg(feature = "plugin-dendanskeordbog")]
  dendanskeordbog::DenDanskeOrdbog => dendanskeordbog::Settings,

  /// Query nameservers
  #[cfg(feature = "plugin-dig")]
  dig::Dig => dig::Settings,

  /// Query geolocation of addresses and hostnames
  #[cfg(feature = "plugin-geoip")]
  geoip::GeoIp => geoip::Settings,

  /// Normative affective resonance profiling, calendar-scoped
  #[cfg(feature = "plugin-gay")]
  gay::Gay => NoSettings,

  /// GitHub integration
  #[cfg(feature = "plugin-github")]
  github::GitHubPlugin => github::Settings,

  /// Process health information
  #[cfg(feature = "plugin-health")]
  health::Health => NoSettings,

  /// List loaded plugins and their commands
  #[cfg(feature = "plugin-help")]
  help::Help => NoSettings,

  /// Howlongtobeat.com integration
  #[cfg(feature = "plugin-howlongtobeat")]
  howlongtobeat::HowLongToBeat => NoSettings,

  /// IMDb integration
  #[cfg(feature = "plugin-imdb")]
  imdb::Imdb => imdb::Settings,

  /// Instagram integration
  #[cfg(feature = "plugin-instagram")]
  instagram::Instagram => instagram::Settings,

  /// Is it open
  #[cfg(feature = "plugin-isitopen")]
  isitopen::IsItOpen => isitopen::Settings,

  /// Kagi search integration
  #[cfg(feature = "plugin-kagi")]
  kagi::KagiPlugin => kagi::Settings,

  /// User notifications
  #[cfg(feature = "plugin-notification")]
  notification::NotificationPlugin => notification::Settings,

  /// URL history
  #[cfg(feature = "plugin-ofn")]
  ofn::Ofn => NoSettings,

  /// Weather service integration
  #[cfg(feature = "plugin-openweathermap")]
  openweathermap::OpenWeatherMap => openweathermap::Settings,

  /// PornHub platform integration
  #[cfg(feature = "plugin-pornhub")]
  pornhub::PornHub => NoSettings,

  /// Reddit plugin integration
  #[cfg(feature = "plugin-reddit")]
  reddit::Reddit => reddit::Settings,

  /// Calculator plugin based on rink
  #[cfg(feature = "plugin-rink")]
  rink::Rink => NoSettings,

  /// Rust Playground integration
  #[cfg(feature = "plugin-rust-playground")]
  rust_playground::RustPlayground => rust_playground::Settings,

  /// Spotify integration
  #[cfg(feature = "plugin-spotify")]
  spotify::Spotify => spotify::Settings,

  /// Generic string utility plugin
  #[cfg(feature = "plugin-string-utils")]
  string_utils::StringUtils => NoSettings,

  /// Thingiverse integration
  #[cfg(feature = "plugin-thingiverse")]
  thingiverse::Thingiverse => thingiverse::Settings,

  /// TikTok integration
  #[cfg(feature = "plugin-tiktok")]
  tiktok::Tiktok => tiktok::Settings,

  /// URL titles and OpenGraph metadata
  #[cfg(feature = "plugin-titles")]
  titles::Titles => titles::Settings,

  /// Trustpilot integration
  #[cfg(feature = "plugin-trustpilot")]
  trustpilot::Trustpilot => trustpilot::Settings,

  /// TVmaze integration
  #[cfg(feature = "plugin-tvmaze")]
  tvmaze::Tvmaze => NoSettings,

  /// Twitch integration
  #[cfg(feature = "plugin-twitch")]
  twitch::Twitch => twitch::Settings,

  /// Urban Dictionary integration
  #[cfg(feature = "plugin-urban-dictionary")]
  urban_dictionary::UrbanDictionary => urban_dictionary::Settings,

  /// YouTube integration
  #[cfg(feature = "plugin-youtube")]
  youtube::YouTube => youtube::Settings,
}

/// Object-safe view of [`Plugin`], implemented automatically for every `Plugin<Context>`.
///
/// [`Plugin`] declares an associated [`Settings`](Plugin::Settings) type and therefore cannot be
/// used directly as a trait object. The registry stores plugins with different settings types
/// behind this trait, which erases the settings type and exposes only the runtime methods the
/// host needs. Plugin authors never implement this trait themselves.
///
/// The methods return the plugin's already-boxed futures directly instead of being declared
/// `async`; forwarding an `async fn` into another `async fn` would allocate a second boxed future
/// that only awaits the first.
pub trait ErasedPlugin: Send + Sync {
    /// Routes an event to the handler registered for its kind.
    ///
    /// Events are only routed to handlers the plugin registered the matching kind for, so an
    /// unknown variant is unreachable in practice; it is logged when a future event kind is
    /// added without a routing arm.
    fn handle_event<'a>(
        &'a self,
        ctx: &'a Context,
        client: &'a Client,
        event: &'a Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;

    /// Called when all plugins are loaded and the client has connected to the network.
    fn loaded<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;

    /// Called once while the bot is shutting down, after the plugin's event queue has been
    /// drained.
    fn shutdown<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;
}

impl<P: Plugin<Context>> ErasedPlugin for P {
    fn handle_event<'a>(
        &'a self,
        ctx: &'a Context,
        client: &'a Client,
        event: &'a Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            match event {
                Event::Command(command) => Plugin::handle_command(self, ctx, client, command).await,
                Event::Url(url) => Plugin::handle_url(self, ctx, client, url).await,
                Event::Message(message) => Plugin::handle_message(self, ctx, client, message).await,
                Event::Join(join) => Plugin::handle_join(self, ctx, client, join).await,
                Event::Part(part) => Plugin::handle_part(self, ctx, client, part).await,
                Event::Quit(quit) => Plugin::handle_quit(self, ctx, client, quit).await,
                Event::Nick(nick) => Plugin::handle_nick(self, ctx, client, nick).await,
                Event::Kick(kick) => Plugin::handle_kick(self, ctx, client, kick).await,
                Event::Ctcp(ctcp) => Plugin::handle_ctcp(self, ctx, client, ctcp).await,
                Event::Raw(raw) => Plugin::handle_raw(self, ctx, client, raw).await,
                // Future event kinds are accepted so existing plugins keep compiling; the
                // dispatcher routes them once their kind gains a routing arm.
                _ => {
                    warn!(plugin = P::NAME, "unhandled event kind");

                    Ok(())
                }
            }
        })
    }

    fn loaded<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Plugin::loaded(self, ctx, client)
    }

    fn shutdown<'a>(
        &'a mut self,
        ctx: &'a Context,
        client: &'a Client,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Plugin::shutdown(self, ctx, client)
    }
}

/// A plugin registered with the bot, together with the subscriptions it declared.
pub struct RegisteredPlugin {
    /// The name of the plugin.
    pub name: String,
    /// The plugin itself.
    pub plugin: Box<dyn ErasedPlugin>,
    /// The events the plugin registered interest in during initialization.
    pub subscriptions: Subscriptions,
}

/// Metadata about a registered plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    /// The name of the plugin.
    pub name: String,
    /// The commands handled by the plugin.
    pub commands: Vec<CommandSpec>,
    /// The URL hosts whose links the plugin handles itself.
    pub url_hosts: Vec<&'static str>,
}

/// Snapshot of the plugins registered with the bot and the events they handle.
///
/// The catalog is published to [`Context::shared`] when the registry is preloaded, so plugins can
/// look it up with [`SharedState::get`] to discover other plugins.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PluginCatalog {
    /// The registered plugins, in registration order.
    pub entries: Vec<CatalogEntry>,
}

/// Plugin registry.
#[derive(Default)]
pub struct Registry {
    /// List of loaded plugins.
    pub plugins: Vec<RegisteredPlugin>,
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
    pub fn preloaded(ctx: &Context, plugins: &PluginsConfig) -> Registry {
        let mut registry = Self::new();
        debug!("registering plugins");

        // The shared media mirror is published before any plugin is constructed, so plugins can
        // pick it up through their constructors.
        #[cfg(feature = "mirror")]
        crate::mirror::publish_shared_mirror(ctx);

        for section in &plugins.unknown {
            if !BUNDLED_PLUGIN_NAMES.contains(&section.0.as_str()) {
                warn!(
                    section = %section.0,
                    "unknown plugin configuration section (typo?)"
                );
            }
        }

        registry.register_bundled_plugins(ctx, plugins);

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
    /// A fresh [`Subscriptions`] set is passed to the plugin's constructor, which registers the
    /// events it handles on it. The subscriptions are committed — to the catalog and to the
    /// returned plugin — only when construction succeeds.
    ///
    /// Returns `true` if the plugin was successfully initialized and registered, `false` if
    /// initialization failed. Failed plugins are tracked in `self.failed` and logged with their
    /// name and error.
    pub fn register<P: Plugin<Context> + 'static>(
        &mut self,
        ctx: &Context,
        settings: &P::Settings,
    ) -> bool {
        let name = P::NAME.to_string();
        let mut subscriptions = Subscriptions::new();

        match P::new(ctx, settings, &mut subscriptions) {
            Ok(plugin) => {
                debug!(plugin = %name, "registered plugin");

                let url_hosts = match subscriptions.url_scope() {
                    zeta_plugin::UrlScope::Hosts(hosts) => hosts.to_vec(),
                    _ => Vec::new(),
                };

                self.catalog.entries.push(CatalogEntry {
                    name: name.clone(),
                    commands: subscriptions.commands().to_vec(),
                    url_hosts,
                });
                self.plugins.push(RegisteredPlugin {
                    name,
                    plugin: Box::new(plugin),
                    subscriptions,
                });

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
    pub fn take_plugins(&mut self) -> Vec<RegisteredPlugin> {
        std::mem::take(&mut self.plugins)
    }
}

/// A plugin running in its own long-lived task.
///
/// The task loads the plugin and then waits for events on an unbounded channel, handling them
/// one at a time. State that other plugins should be able to access must be published to
/// [`Context::shared`] when the plugin is constructed or loaded.
///
/// Closing the mailbox — by dropping its senders, which the dispatcher holds — makes the plugin
/// drain its queued events, run the shutdown hook and exit.
pub struct PluginTask {
    /// The name of the plugin, used for logging.
    pub name: String,
    /// The handle of the spawned plugin task.
    handle: JoinHandle<()>,
}

impl PluginTask {
    /// Spawns `plugin` into a task that loads it and processes events until the channel closes.
    ///
    /// If the plugin fails to load, the error is logged and the task exits. When the channel
    /// closes, the plugin's shutdown hook runs before the task exits.
    pub fn spawn(
        name: String,
        mut plugin: Box<dyn ErasedPlugin>,
        ctx: Arc<Context>,
        client: Arc<Client>,
        mut receiver: mpsc::UnboundedReceiver<Event>,
    ) -> Self {
        let task_name = name.clone();

        let handle = tokio::spawn(async move {
            debug!(plugin = %task_name, "plugin task started");

            // The load and shutdown hooks run inside spans for the same reason the handler
            // does: without a span, their warnings only ever reach stdout — the `loaded` span
            // above closed before this one fires.
            let failed_to_load = async {
                if let Err(error) = plugin.loaded(&ctx, &client).await {
                    warn!(plugin = %task_name, %error, "plugin failed to load");

                    true
                } else {
                    false
                }
            }
            .instrument(tracing::info_span!("loaded", plugin = %task_name))
            .await;

            if failed_to_load {
                return;
            }

            while let Some(event) = receiver.recv().await {
                // The handler runs inside a span: the OpenTelemetry layer drops events that
                // are not in the context of a span, so without it the plugin's logs would
                // only ever reach stdout.
                async {
                    if let Err(error) = plugin.handle_event(&ctx, &client, &event).await {
                        error!(plugin = %task_name, %error, "plugin error during event handling");
                    }
                }
                .instrument(tracing::info_span!("handle_event", plugin = %task_name))
                .await;
            }

            // The mailbox is closed: the bot is shutting down. The warning goes inside the
            // span, for the same reason as the load one above.
            async {
                if let Err(error) = plugin.shutdown(&ctx, &client).await {
                    warn!(plugin = %task_name, %error, "plugin error during shutdown");
                }
            }
            .instrument(tracing::info_span!("shutdown", plugin = %task_name))
            .await;

            debug!(plugin = %task_name, "plugin task stopped");
        });

        Self { name, handle }
    }

    /// Consumes the plugin task and returns its name with the task's handle.
    ///
    /// The mailbox is closed by the dispatcher, which holds the sending end; awaiting the
    /// returned handle — bounded by a deadline — waits for the plugin to drain its queue and
    /// run its shutdown hook.
    #[must_use]
    pub fn into_handle(self) -> (String, JoinHandle<()>) {
        (self.name, self.handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use async_trait::async_trait;
    use figment::Figment;
    use figment::providers::{Format, Toml};
    use irc::proto::Message;
    use zeta_plugin::MessageEvent;

    #[test]
    fn bundled_plugin_names_include_all_plugins() {
        assert_eq!(BUNDLED_PLUGIN_NAMES.len(), 34);
        assert!(BUNDLED_PLUGIN_NAMES.contains(&"filter"));
        assert!(BUNDLED_PLUGIN_NAMES.contains(&"dig"));
        assert!(BUNDLED_PLUGIN_NAMES.contains(&"health"));
        assert!(BUNDLED_PLUGIN_NAMES.contains(&"howlongtobeat"));
    }

    /// The lifecycle log shared between a test and its plugin through the context.
    type Recording = Arc<std::sync::Mutex<Vec<&'static str>>>;

    /// Publishes the recording log through the context's shared state.
    fn publish_recording(ctx: &Context) -> Recording {
        let recording: Recording = Arc::new(std::sync::Mutex::new(Vec::new()));

        assert!(ctx.shared.publish(Arc::clone(&recording)).is_none());

        recording
    }

    /// Test plugin recording the lifecycle events it observes.
    struct RecordingPlugin {
        recording: Recording,
    }

    impl zeta_plugin::PluginName for RecordingPlugin {
        const NAME: &'static str = "recording";
    }

    #[async_trait]
    impl Plugin<Context> for RecordingPlugin {
        type Settings = NoSettings;

        fn new(ctx: &Context, _: &NoSettings, _: &mut Subscriptions) -> Result<Self, Error> {
            let recording = ctx
                .shared
                .get::<std::sync::Mutex<Vec<&'static str>>>()
                .expect("recording log should be published");

            Ok(RecordingPlugin { recording })
        }

        async fn handle_message(
            &self,
            _: &Context,
            _: &Client,
            _: &MessageEvent,
        ) -> Result<(), Error> {
            crate::sync::lock(&self.recording).push("message");

            Ok(())
        }

        async fn shutdown(&mut self, _: &Context, _: &Client) -> Result<(), Error> {
            crate::sync::lock(&self.recording).push("shutdown");

            Ok(())
        }
    }

    #[cfg(feature = "database")]
    #[tokio::test]
    async fn plugin_task_drains_messages_before_shutdown() {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://invalid/zeta_test")
            .expect("lazy database pool");

        let mut config: Config = Figment::new()
            .merge(Toml::string(
                r#"
[database]
url = "postgresql://invalid/zeta_test"

[tracing]
enabled = false

[irc]
nickname = "zeta-test"
hostname = "mock"
alt_nicks = []
channels = []
"#,
            ))
            .extract()
            .expect("test configuration should parse");

        let _ = config.take_plugins();

        let ctx = Arc::new(Context::new(db, crate::dns::new(), config));
        let recording = publish_recording(&ctx);

        let client = Arc::new(
            Client::from_config(irc::client::data::Config {
                nickname: Some("zeta-test".to_owned()),
                server: Some("mock".to_owned()),
                use_mock_connection: true,
                mock_initial_value: Some(String::new()),
                ..Default::default()
            })
            .await
            .expect("mock irc client"),
        );

        let (sender, receiver) = mpsc::unbounded_channel();
        let task = PluginTask::spawn(
            "recording".to_string(),
            Box::new(RecordingPlugin {
                recording: Arc::clone(&recording),
            }),
            Arc::clone(&ctx),
            Arc::clone(&client),
            receiver,
        );

        for _ in 0..2 {
            let message = Message::new(None, "PRIVMSG", vec!["#test", "hello"])
                .expect("message should parse");

            sender
                .send(Event::Message(MessageEvent::new(Arc::new(message))))
                .expect("task should be running");
        }

        // Closing the mailbox makes the task drain its queue, run the shutdown hook and exit.
        drop(sender);

        let (_, handle) = task.into_handle();
        handle.await.expect("task should finish");

        let log = crate::sync::lock(&recording);

        assert_eq!(*log, ["message", "message", "shutdown"]);
    }
}
