//! The bundled plugins for the zeta IRC bot.
//!
//! Every plugin lives in its own `plugins-<name>` crate and implements
//! [`Plugin`](zeta_plugin::Plugin) from `zeta-plugin` through the `#[plugin]` attribute macro,
//! which also registers the plugin with the host through `inventory`.
//! [`register_bundled_plugins`] discovers the compiled-in plugins from those registrations,
//! resolves each one's section in [`PluginsConfig`], and registers the enabled ones with
//! [`Registry`](zeta_plugin_runtime::Registry).
//!
//! This crate is the facade over those crates: its cargo features select which plugin crates are
//! compiled in, preserving the feature names the host has always used.
//!
//! # Features
//!
//! `all` (the default) enables every bundled plugin. Each plugin also has a feature named after
//! it (`titles`, `youtube`, `unwall`, ...); the shared `database`, `http`, `emulated`, `mirror`
//! and `time` features group the capabilities the plugin crates need. See `Cargo.toml` for the
//! exact graph.

pub mod config;

pub use config::{PluginSection, PluginsConfig};

// Re-export the enabled plugin crates so their `inventory` registrations are linked into the
// facade and any binary that uses it. An unreferenced optional dependency is not necessarily
// linked, which would silently drop its plugin.
#[cfg(feature = "alert")]
pub use plugins_alert as alert;
#[cfg(feature = "chaturbate")]
pub use plugins_chaturbate as chaturbate;
#[cfg(feature = "choices")]
pub use plugins_choices as choices;
#[cfg(feature = "coinmarketcap")]
pub use plugins_coinmarketcap as coinmarketcap;
#[cfg(feature = "dendanskeordbog")]
pub use plugins_dendanskeordbog as dendanskeordbog;
#[cfg(feature = "dig")]
pub use plugins_dig as dig;
#[cfg(feature = "filter")]
pub use plugins_filter as filter;
#[cfg(feature = "gay")]
pub use plugins_gay as gay;
#[cfg(feature = "geoip")]
pub use plugins_geoip as geoip;
#[cfg(feature = "github")]
pub use plugins_github as github;
#[cfg(feature = "health")]
pub use plugins_health as health;
#[cfg(feature = "help")]
pub use plugins_help as help;
#[cfg(feature = "howlongtobeat")]
pub use plugins_howlongtobeat as howlongtobeat;
#[cfg(feature = "imdb")]
pub use plugins_imdb as imdb;
#[cfg(feature = "instagram")]
pub use plugins_instagram as instagram;
#[cfg(feature = "isitopen")]
pub use plugins_isitopen as isitopen;
#[cfg(feature = "kagi")]
pub use plugins_kagi as kagi;
#[cfg(feature = "notification")]
pub use plugins_notification as notification;
#[cfg(feature = "ofn")]
pub use plugins_ofn as ofn;
#[cfg(feature = "openweathermap")]
pub use plugins_openweathermap as openweathermap;
#[cfg(feature = "pornhub")]
pub use plugins_pornhub as pornhub;
#[cfg(feature = "reddit")]
pub use plugins_reddit as reddit;
#[cfg(feature = "rink")]
pub use plugins_rink as rink;
#[cfg(feature = "rust-playground")]
pub use plugins_rust_playground as rust_playground;
#[cfg(feature = "spotify")]
pub use plugins_spotify as spotify;
#[cfg(feature = "string-utils")]
pub use plugins_string_utils as string_utils;
#[cfg(feature = "thingiverse")]
pub use plugins_thingiverse as thingiverse;
#[cfg(feature = "tiktok")]
pub use plugins_tiktok as tiktok;
#[cfg(feature = "titles")]
pub use plugins_titles as titles;
#[cfg(feature = "trustpilot")]
pub use plugins_trustpilot as trustpilot;
#[cfg(feature = "tvmaze")]
pub use plugins_tvmaze as tvmaze;
#[cfg(feature = "twitch")]
pub use plugins_twitch as twitch;
#[cfg(feature = "unwall")]
pub use plugins_unwall as unwall;
#[cfg(feature = "urban-dictionary")]
pub use plugins_urban_dictionary as urban_dictionary;
#[cfg(feature = "youtube")]
pub use plugins_youtube as youtube;

/// The names of every bundled plugin, whether or not its cargo feature is enabled.
///
/// Configuration sections for these names describe a plugin that was compiled out, not a typo,
/// so the host can report the two cases differently.
pub(crate) const BUNDLED_PLUGIN_NAMES: &[&str] = &[
    "alert",
    "chaturbate",
    "choices",
    "coinmarketcap",
    "dendanskeordbog",
    "dig",
    "filter",
    "gay",
    "geoip",
    "github",
    "health",
    "help",
    "howlongtobeat",
    "imdb",
    "instagram",
    "isitopen",
    "kagi",
    "notification",
    "ofn",
    "openweathermap",
    "pornhub",
    "reddit",
    "rink",
    "rust_playground",
    "spotify",
    "string_utils",
    "thingiverse",
    "tiktok",
    "titles",
    "trustpilot",
    "tvmaze",
    "twitch",
    "unwall",
    "urban_dictionary",
    "youtube",
];

/// Registers every compiled-in bundled plugin that its configuration enables.
///
/// The host calls this once at startup, before the catalog is published. Plugins register
/// themselves through `inventory` when their `#[plugin]` implementation is linked in, so this
/// function only has to iterate the registrations — sorted by name for a deterministic catalog
/// and log order.
///
/// Configuration sections matching no bundled plugin are warned about as likely typos; a section
/// for a bundled plugin whose cargo feature is disabled is known, but has no registration.
/// Plugins that fail to initialize are logged and skipped by the registry.
pub fn register_bundled_plugins(
    registry: &mut zeta_plugin_runtime::Registry,
    ctx: &zeta_plugin::Context,
    plugins: &PluginsConfig,
) {
    for name in &plugins.unknown {
        tracing::warn!(section = %name, "unknown plugin configuration section (typo?)");
    }

    for (plugin, key) in &plugins.unknown_setting_keys {
        tracing::warn!(
            plugin = %plugin,
            key = %key,
            "unknown settings key (typo, or a setting removed in this version?)"
        );
    }

    // Sorted by name for a deterministic catalog and log order. Duplicate names are a build
    // error and are reported here regardless of whether the plugin is enabled; the registry
    // rejects them again for callers that bypass this function.
    let mut registrations: Vec<&'static zeta_plugin::PluginRegistration> =
        zeta_plugin::inventory::iter::<zeta_plugin::PluginRegistration>().collect();
    registrations.sort_unstable_by_key(|registration| registration.name);

    let mut previous: Option<&'static str> = None;
    for registration in registrations {
        if previous == Some(registration.name) {
            tracing::error!(
                plugin = registration.name,
                "duplicate plugin registration; the later one is ignored"
            );
            continue;
        }
        previous = Some(registration.name);

        let (enabled, section) = plugins
            .sections
            .get(registration.name)
            .map_or((plugins.enabled_by_default, None), |section| {
                (section.enabled, Some(&section.settings))
            });

        if enabled {
            registry.register_with(registration, ctx, section);
        } else {
            tracing::info!(
                plugin = registration.name,
                "plugin is disabled by configuration"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    #[cfg(feature = "all")]
    use std::collections::BTreeSet;

    #[cfg(feature = "all")]
    use crate::BUNDLED_PLUGIN_NAMES;

    /// The crate roots under `src`: `lib.rs`, `main.rs`, and the implicit binaries in `src/bin/`.
    ///
    /// Only the conventional layout is recognized. The workspace's explicit `[[bin]]` path
    /// points at `src/main.rs`, which this covers; a future custom root would need extending
    /// here.
    fn crate_roots(src: &Path) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = ["lib.rs", "main.rs"]
            .into_iter()
            .map(|name| src.join(name))
            .filter(|path| path.is_file())
            .collect();

        let bin = src.join("bin");
        if bin.is_dir() {
            for entry in std::fs::read_dir(&bin).expect("bin directory") {
                let path = entry.expect("directory entry").path();

                if path.extension().is_some_and(|extension| extension == "rs") {
                    roots.push(path);
                } else if path.join("main.rs").is_file() {
                    roots.push(path.join("main.rs"));
                }
            }
        }

        roots
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

    /// Every source file under `crates/*/src` must be declared as a module by its parent: an
    /// undeclared file is dead on disk — cargo compiles it never, and nothing else flags it. The
    /// check parses the files as Rust items, so feature-gated modules count whether or not the
    /// feature is enabled.
    ///
    /// # Panics
    ///
    /// Panics if a crate has no conventional root, if a source file cannot be read or parsed, or
    /// if a source file is not declared.
    #[test]
    fn every_source_file_is_declared_as_a_module() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the facade lives under crates/");

        let mut orphans = Vec::new();

        for entry in std::fs::read_dir(crates).expect("read the crates directory") {
            let krate = entry.expect("directory entry").path();
            let src = krate.join("src");

            if !src.is_dir() {
                continue;
            }

            let mut declared: HashSet<PathBuf> = crate_roots(&src).into_iter().collect();

            assert!(
                !declared.is_empty(),
                "{} has no lib.rs, main.rs, or src/bin root",
                krate.display()
            );

            let mut existing = HashSet::new();
            let mut stack = vec![src];

            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).expect("source directory") {
                    let path = entry.expect("directory entry").path();

                    if path.is_dir() {
                        stack.push(path);
                        continue;
                    }

                    if path.extension().is_none_or(|extension| extension != "rs") {
                        continue;
                    }

                    existing.insert(path.clone());

                    let source = std::fs::read_to_string(&path).expect("read source file");

                    // The submodules of `foo.rs` live in `foo/` beside it, while `lib.rs`,
                    // `main.rs` and `mod.rs` hold the modules of their own directory.
                    let stem = path
                        .file_stem()
                        .expect("rs file has a stem")
                        .to_string_lossy();
                    let child_dir = match stem.as_ref() {
                        "lib" | "main" | "mod" => {
                            path.parent().expect("rs file has a parent").to_path_buf()
                        }
                        _ => path.with_file_name(stem.as_ref()),
                    };

                    for name in declared_modules(&source) {
                        declared.insert(child_dir.join(format!("{name}.rs")));
                        declared.insert(child_dir.join(name).join("mod.rs"));
                    }
                }
            }

            orphans.extend(existing.difference(&declared).cloned());
        }

        orphans.sort();

        assert!(
            orphans.is_empty(),
            "source files no module declares (dead on disk): {orphans:?}"
        );
    }

    /// Every bundled plugin self-registers through the `#[plugin]` macro, independently of any
    /// central list. `--all-features` compiles all 35 of them.
    #[cfg(feature = "all")]
    #[test]
    fn all_bundled_plugins_register_themselves() {
        assert_eq!(
            zeta_plugin::inventory::iter::<zeta_plugin::PluginRegistration>().count(),
            35
        );
    }

    /// Every name the host knows about has a registration when every plugin is compiled in.
    #[cfg(feature = "all")]
    #[test]
    fn every_bundled_plugin_name_has_a_registration() {
        let registered: BTreeSet<&str> =
            zeta_plugin::inventory::iter::<zeta_plugin::PluginRegistration>()
                .map(|registration| registration.name)
                .collect();
        let bundled: BTreeSet<&str> = BUNDLED_PLUGIN_NAMES.iter().copied().collect();

        assert_eq!(registered, bundled);
    }
}
