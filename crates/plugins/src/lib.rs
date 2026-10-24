//! The bundled plugins for the zeta IRC bot.
//!
//! Every plugin implements [`Plugin`](zeta_plugin::Plugin) from `zeta-plugin`: it registers the
//! events it wants to receive in its constructor and handles them through the per-kind handler
//! methods. The `declare_plugins!` invocation below ties the plugins together into the typed
//! [`PluginsConfig`] configuration and the [`register_bundled_plugins`] entry point the host
//! calls at startup.

pub mod config;

/// Database helpers shared by the DB-backed plugins.
#[cfg(feature = "database")]
pub(crate) mod database;

mod prelude;

/// Admin hostmask matching shared by the plugins that gate commands on `[irc] admin_hostmasks`.
#[cfg(feature = "filter")]
pub(crate) mod hostmask;

/// The reply flow shared by the mirroring plugins.
#[cfg(feature = "mirror")]
pub(crate) mod mirror_reply;

pub use config::PluginConfig;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use zeta_plugin::NoSettings;

/// Declares the bundled plugins and generates the configuration and registration helpers.
///
/// The plugin modules themselves are declared literally above this invocation so rustfmt can
/// see them. For each entry, the macro generates:
/// 1. A [`zeta_plugin::PluginName`] implementation for the plugin's struct.
/// 2. A field in [`PluginsConfig`] holding the plugin's settings type: `mod::Struct => mod::Settings`
///    for plugins with settings, `=> NoSettings` otherwise.
/// 3. A call to [`zeta_plugin::Registry::register`] inside [`register_bundled_plugins`], gated on
///    the plugin's `enabled` configuration and passing the plugin's own settings through to its
///    constructor.
macro_rules! declare_plugins {
    (
        $(
            $(#[doc = $doc:expr])*
            #[cfg(feature = $feature:literal)]
            $mod_name:ident :: $struct_name:ident => $settings:ty
        ),* $(,)?
    ) => {
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
        /// entirely takes the global [`PluginsConfig::enabled_by_default`] with default
        /// settings. The `enabled` key is managed by the host; every other key belongs to the
        /// plugin's settings type.
        ///
        /// Keys under `[plugins]` that match neither the `enabled_by_default` toggle nor a
        /// bundled plugin name are collected in [`PluginsConfig::unknown`] so the host can warn
        /// about likely typos. Unknown keys inside a section are ignored and reported through
        /// [`PluginConfig::unknown_keys`](config::PluginConfig::unknown_keys), which the
        /// host also warns about.
        #[derive(Clone, Debug, Serialize)]
        pub struct PluginsConfig {
            /// Whether a plugin that does not set `enabled` explicitly — or has no section at
            /// all — is enabled.
            ///
            /// `true` (the default) makes plugins opt-out: every bundled plugin is enabled
            /// unless its section says `enabled = false`. `false` makes them opt-in: a plugin
            /// only runs when its section says `enabled = true`.
            pub enabled_by_default: bool,

            $(
                $(#[doc = $doc])*
                #[cfg(feature = $feature)]
                pub $mod_name: $crate::config::PluginConfig<$settings>,
            )*

            /// Sections that match no bundled plugin, whether compiled in or not.
            #[serde(flatten)]
            pub unknown: HashMap<String, figment::value::Dict>,
        }

        impl Default for PluginsConfig {
            fn default() -> Self {
                Self {
                    enabled_by_default: true,
                    $(
                        #[cfg(feature = $feature)]
                        $mod_name: $crate::config::PluginConfig::default(),
                    )*
                    unknown: HashMap::new(),
                }
            }
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

                #[allow(unused_mut)]
                let mut sections =
                    HashMap::<String, figment::value::Value>::deserialize(deserializer)?;

                // The global opt-in/opt-out toggle is pulled out first so it is not reported
                // as an unknown section.
                let enabled_by_default = match sections.remove("enabled_by_default") {
                    Some(value) => bool::deserialize(&value).map_err(|error| {
                        D::Error::custom(format!(
                            "invalid `enabled_by_default` key: {error}"
                        ))
                    })?,
                    None => true,
                };
                #[allow(unused_mut)]
                let mut config = Self {
                    enabled_by_default,
                    ..Self::default()
                };

                $(
                    #[cfg(feature = $feature)]
                    if let Some(section) = sections.remove(stringify!($mod_name)) {
                        config.$mod_name = $crate::config::PluginConfig::<$settings>::deserialize_with_default(
                            &section,
                            enabled_by_default,
                        )
                        .map_err(|error| {
                            D::Error::custom(format!(
                                "invalid `{}` plugin section: {error}",
                                stringify!($mod_name)
                            ))
                        })?;
                    } else {
                        // A section that is omitted entirely follows the global default too,
                        // so an opt-in configuration really leaves every unlisted plugin out.
                        config.$mod_name.enabled = enabled_by_default;
                    }
                )*

                for (name, section) in sections {
                    let section = figment::value::Dict::deserialize(&section).map_err(|error| {
                        D::Error::custom(format!("invalid `{name}` plugin section: {error}"))
                    })?;

                    config.unknown.insert(name, section);
                }

                Ok(config)
            }
        }

        /// The names of every bundled plugin, compiled in or not.
        pub const BUNDLED_PLUGIN_NAMES: &[&str] = &[
            $(
                stringify!($mod_name),
            )*
        ];


        /// Registers every compiled-in bundled plugin that its configuration enables.
        ///
        /// The host calls this once at startup, before the catalog is published; plugins that
        /// fail to initialize are logged and skipped by the registry. Configuration sections
        /// matching no bundled plugin are warned about as likely typos.
        pub fn register_bundled_plugins(
            #[allow(unused)] registry: &mut zeta_plugin::Registry,
            #[allow(unused)] ctx: &zeta_plugin::Context,
            plugins: &PluginsConfig,
        ) {
            for section in &plugins.unknown {
                if !BUNDLED_PLUGIN_NAMES.contains(&section.0.as_str()) {
                    ::tracing::warn!(
                        section = %section.0,
                        "unknown plugin configuration section (typo?)"
                    );
                }
            }

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
                        registry.register::<$mod_name::$struct_name>(ctx, &section.settings);
                    } else {
                        ::tracing::info!(
                            plugin = stringify!($mod_name),
                            "plugin is disabled by configuration"
                        );
                    }
                }
            )*
        }
    };
}

// The bundled plugin modules are declared literally, outside `declare_plugins!`, so
// rustfmt can discover them. Rustfmt does not expand macros, so a generated `pub mod`
// would leave every plugin file invisible to `cargo fmt`.
#[cfg(feature = "alert")]
pub mod alert;
#[cfg(feature = "chaturbate")]
pub mod chaturbate;
#[cfg(feature = "choices")]
pub mod choices;
#[cfg(feature = "coinmarketcap")]
pub mod coinmarketcap;
#[cfg(feature = "dendanskeordbog")]
pub mod dendanskeordbog;
#[cfg(feature = "dig")]
pub mod dig;
#[cfg(feature = "filter")]
pub mod filter;
#[cfg(feature = "gay")]
pub mod gay;
#[cfg(feature = "geoip")]
pub mod geoip;
#[cfg(feature = "github")]
pub mod github;
#[cfg(feature = "health")]
pub mod health;
#[cfg(feature = "help")]
pub mod help;
#[cfg(feature = "howlongtobeat")]
pub mod howlongtobeat;
#[cfg(feature = "imdb")]
pub mod imdb;
#[cfg(feature = "instagram")]
pub mod instagram;
#[cfg(feature = "isitopen")]
pub mod isitopen;
#[cfg(feature = "kagi")]
pub mod kagi;
#[cfg(feature = "notification")]
pub mod notification;
#[cfg(feature = "ofn")]
pub mod ofn;
#[cfg(feature = "openweathermap")]
pub mod openweathermap;
#[cfg(feature = "pornhub")]
pub mod pornhub;
#[cfg(feature = "reddit")]
pub mod reddit;
#[cfg(feature = "rink")]
pub mod rink;
#[cfg(feature = "rust-playground")]
pub mod rust_playground;
#[cfg(feature = "spotify")]
pub mod spotify;
#[cfg(feature = "string-utils")]
pub mod string_utils;
#[cfg(feature = "thingiverse")]
pub mod thingiverse;
#[cfg(feature = "tiktok")]
pub mod tiktok;
#[cfg(feature = "titles")]
pub mod titles;
#[cfg(feature = "trustpilot")]
pub mod trustpilot;
#[cfg(feature = "tvmaze")]
pub mod tvmaze;
#[cfg(feature = "twitch")]
pub mod twitch;
#[cfg(feature = "unwall")]
pub mod unwall;
#[cfg(feature = "urban-dictionary")]
pub mod urban_dictionary;
#[cfg(feature = "youtube")]
pub mod youtube;

declare_plugins! {
  /// Channel-scoped URL and sender filters.
  #[cfg(feature = "filter")]
  filter::FilterPlugin => NoSettings,

  /// Time-based user alerts.
  #[cfg(feature = "alert")]
  alert::AlertPlugin => alert::Settings,

  /// Chaturbate platform integration.
  #[cfg(feature = "chaturbate")]
  chaturbate::Chaturbate => NoSettings,

  /// Plugin that helps the user make a choice
  #[cfg(feature = "choices")]
  choices::Choices => choices::Settings,

  /// Crypto currency quotes via CoinMarketCap
  #[cfg(feature = "coinmarketcap")]
  coinmarketcap::CoinMarketCap => coinmarketcap::Settings,

  /// Query the danish dictionary
  #[cfg(feature = "dendanskeordbog")]
  dendanskeordbog::DenDanskeOrdbog => dendanskeordbog::Settings,

  /// Query nameservers
  #[cfg(feature = "dig")]
  dig::Dig => dig::Settings,

  /// Query geolocation of addresses and hostnames
  #[cfg(feature = "geoip")]
  geoip::GeoIp => geoip::Settings,

  /// Normative affective resonance profiling, calendar-scoped
  #[cfg(feature = "gay")]
  gay::Gay => NoSettings,

  /// GitHub integration
  #[cfg(feature = "github")]
  github::GitHubPlugin => github::Settings,

  /// Process health information
  #[cfg(feature = "health")]
  health::Health => NoSettings,

  /// List loaded plugins and their commands
  #[cfg(feature = "help")]
  help::Help => NoSettings,

  /// Howlongtobeat.com integration
  #[cfg(feature = "howlongtobeat")]
  howlongtobeat::HowLongToBeat => NoSettings,

  /// IMDb integration
  #[cfg(feature = "imdb")]
  imdb::Imdb => imdb::Settings,

  /// Instagram integration
  #[cfg(feature = "instagram")]
  instagram::Instagram => instagram::Settings,

  /// Is it open
  #[cfg(feature = "isitopen")]
  isitopen::IsItOpen => isitopen::Settings,

  /// Kagi search integration
  #[cfg(feature = "kagi")]
  kagi::KagiPlugin => kagi::Settings,

  /// User notifications
  #[cfg(feature = "notification")]
  notification::NotificationPlugin => notification::Settings,

  /// URL history
  #[cfg(feature = "ofn")]
  ofn::Ofn => NoSettings,

  /// Weather service integration
  #[cfg(feature = "openweathermap")]
  openweathermap::OpenWeatherMap => openweathermap::Settings,

  /// PornHub platform integration
  #[cfg(feature = "pornhub")]
  pornhub::PornHub => NoSettings,

  /// Reddit plugin integration
  #[cfg(feature = "reddit")]
  reddit::Reddit => reddit::Settings,

  /// Calculator plugin based on rink
  #[cfg(feature = "rink")]
  rink::Rink => rink::Settings,

  /// Rust Playground integration
  #[cfg(feature = "rust-playground")]
  rust_playground::RustPlayground => rust_playground::Settings,

  /// Spotify integration
  #[cfg(feature = "spotify")]
  spotify::Spotify => spotify::Settings,

  /// Generic string utility plugin
  #[cfg(feature = "string-utils")]
  string_utils::StringUtils => NoSettings,

  /// Thingiverse integration
  #[cfg(feature = "thingiverse")]
  thingiverse::Thingiverse => thingiverse::Settings,

  /// TikTok integration
  #[cfg(feature = "tiktok")]
  tiktok::Tiktok => tiktok::Settings,

  /// URL titles and OpenGraph metadata
  #[cfg(feature = "titles")]
  titles::Titles => titles::Settings,

  /// Trustpilot integration
  #[cfg(feature = "trustpilot")]
  trustpilot::Trustpilot => trustpilot::Settings,

  /// TVmaze integration
  #[cfg(feature = "tvmaze")]
  tvmaze::Tvmaze => NoSettings,

  /// Twitch integration
  #[cfg(feature = "twitch")]
  twitch::Twitch => twitch::Settings,

  /// Urban Dictionary integration
  #[cfg(feature = "urban-dictionary")]
  urban_dictionary::UrbanDictionary => urban_dictionary::Settings,

  /// Paywalled news links mirrored through unwall.app
  #[cfg(feature = "unwall")]
  unwall::Unwall => unwall::Settings,

  /// YouTube integration
  #[cfg(feature = "youtube")]
  youtube::YouTube => youtube::Settings,
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use super::*;

    #[test]
    fn bundled_plugin_names_include_all_plugins() {
        assert_eq!(BUNDLED_PLUGIN_NAMES.len(), 35);
    }

    /// The feature named by `#[cfg(feature = "...")]`, if the attribute has that shape.
    fn feature_gate(attrs: &[syn::Attribute]) -> Option<String> {
        attrs.iter().find_map(|attribute| {
            if !attribute.path().is_ident("cfg") {
                return None;
            }
            let mut feature = None;
            attribute
                .parse_nested_meta(|meta| {
                    if meta.path.is_ident("feature") {
                        feature = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                    }
                    Ok(())
                })
                .ok()?;
            feature
        })
    }

    /// The file modules (`mod name;`, not `mod name { ... }`) declared in `source`.
    fn declared_modules(source: &str) -> Vec<String> {
        syn::parse_file(source)
            .expect("a parsable Rust source file")
            .items
            .into_iter()
            .filter_map(|item| match item {
                syn::Item::Mod(module) if module.content.is_none() => {
                    Some(module.ident.to_string())
                }
                _ => None,
            })
            .collect()
    }

    /// The public, feature-gated `pub mod name;` declarations in `source`.
    fn plugin_modules(source: &str) -> BTreeSet<String> {
        syn::parse_file(source)
            .expect("a parsable Rust source file")
            .items
            .into_iter()
            .filter_map(|item| {
                let syn::Item::Mod(module) = item else {
                    return None;
                };
                if module.content.is_some()
                    || !matches!(module.vis, syn::Visibility::Public(_))
                    || feature_gate(&module.attrs).is_none()
                {
                    return None;
                }
                Some(module.ident.to_string())
            })
            .collect()
    }

    /// The module of every `module::Struct => settings` entry in the `declare_plugins!`
    /// invocation.
    fn registered_modules(source: &str) -> BTreeSet<String> {
        /// One `module::Struct => settings` macro entry.
        struct Entry {
            module: syn::Ident,
        }

        impl syn::parse::Parse for Entry {
            fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
                let _ = input.call(syn::Attribute::parse_outer)?;
                let module = input.parse()?;
                input.parse::<syn::Token![::]>()?;
                let _ = input.parse::<syn::Ident>()?;
                input.parse::<syn::Token![=>]>()?;
                let _ = input.parse::<syn::Type>()?;
                Ok(Self { module })
            }
        }

        let file = syn::parse_file(source).expect("a parsable Rust source file");
        let invocation = file
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Macro(item)
                    if item.ident.is_none() && item.mac.path.is_ident("declare_plugins") =>
                {
                    Some(&item.mac)
                }
                _ => None,
            })
            .expect("the declare_plugins! invocation");
        invocation
            .parse_body_with(syn::punctuated::Punctuated::<Entry, syn::Token![,]>::parse_terminated)
            .expect("parseable declare_plugins! entries")
            .into_iter()
            .map(|entry| entry.module.to_string())
            .collect()
    }

    /// The literal plugin module list and the `declare_plugins!` entries are two views of the
    /// same registry: rustfmt needs literal `pub mod` declarations, and the macro needs entries
    /// for the `PluginName` impls, configuration and registration. Keep them in step.
    #[test]
    fn plugin_module_declarations_match_registry_entries() {
        let source = include_str!("lib.rs");
        assert_eq!(plugin_modules(source), registered_modules(source));
    }

    /// Every source file under `src` must be declared as a module by its parent: an undeclared
    /// file is dead on disk — cargo compiles it never, and nothing else flags it. The check
    /// parses the files as Rust items, so feature-gated modules count whether or not the
    /// feature is enabled.
    #[test]
    fn every_source_file_is_declared_as_a_module() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut declared = HashSet::from([src.join("lib.rs")]);
        let mut existing = HashSet::new();

        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("source directory") {
                let path = entry.expect("directory entry").path();

                if path.is_dir() {
                    stack.push(path);
                    continue;
                }

                existing.insert(path.clone());

                if path.extension().is_some_and(|extension| extension == "rs") {
                    let source = std::fs::read_to_string(&path).expect("read source file");

                    // The submodules of `foo.rs` live in `foo/` beside it; `lib.rs` and
                    // `mod.rs` hold the modules of their own directory.
                    let stem = path
                        .file_stem()
                        .expect("rs file has a stem")
                        .to_string_lossy();
                    let child_dir = match stem.as_ref() {
                        "lib" | "mod" => path.parent().expect("rs file has a parent").to_path_buf(),
                        _ => path.with_file_name(stem.as_ref()),
                    };

                    for name in declared_modules(&source) {
                        declared.insert(child_dir.join(format!("{name}.rs")));
                        declared.insert(child_dir.join(name).join("mod.rs"));
                    }
                }
            }
        }

        let orphans: Vec<_> = existing.difference(&declared).collect();

        assert!(
            orphans.is_empty(),
            "source files no module declares (dead on disk): {orphans:?}"
        );
    }
}
