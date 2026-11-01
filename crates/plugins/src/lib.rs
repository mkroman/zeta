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
//! it (`titles`, `youtube`, `unwall`, ...); the shared `database` and `http` features group the
//! capabilities the plugin crates need. See `Cargo.toml` for the exact graph.

use std::collections::BTreeSet;

pub mod config;

pub use config::{PluginSection, PluginsConfig};

// Declares each bundled plugin crate once: the re-export that links it (and with it the
// crate's `inventory` registration) and its entry in `BUNDLED_PLUGIN_NAMES`. An unreferenced
// optional dependency is not necessarily linked, which would silently drop its plugin.
macro_rules! bundled_plugins {
    ($($feature:literal => $krate:ident as $name:ident),+ $(,)?) => {
        $(
            #[cfg(feature = $feature)]
            pub use $krate as $name;
        )+

        /// The names of every bundled plugin, whether or not its cargo feature is enabled.
        ///
        /// Configuration sections for these names describe a plugin that was compiled out,
        /// not a typo, so the host can report the two cases differently.
        pub(crate) const BUNDLED_PLUGIN_NAMES: &[&str] = &[$(stringify!($name)),+];
    };
}

bundled_plugins! {
    "alert" => plugins_alert as alert,
    "chaturbate" => plugins_chaturbate as chaturbate,
    "choices" => plugins_choices as choices,
    "coinmarketcap" => plugins_coinmarketcap as coinmarketcap,
    "dendanskeordbog" => plugins_dendanskeordbog as dendanskeordbog,
    "dig" => plugins_dig as dig,
    "filter" => plugins_filter as filter,
    "gay" => plugins_gay as gay,
    "geoip" => plugins_geoip as geoip,
    "github" => plugins_github as github,
    "health" => plugins_health as health,
    "help" => plugins_help as help,
    "howlongtobeat" => plugins_howlongtobeat as howlongtobeat,
    "imdb" => plugins_imdb as imdb,
    "instagram" => plugins_instagram as instagram,
    "isitopen" => plugins_isitopen as isitopen,
    "kagi" => plugins_kagi as kagi,
    "notification" => plugins_notification as notification,
    "ofn" => plugins_ofn as ofn,
    "openweathermap" => plugins_openweathermap as openweathermap,
    "pornhub" => plugins_pornhub as pornhub,
    "reddit" => plugins_reddit as reddit,
    "rink" => plugins_rink as rink,
    "rust-playground" => plugins_rust_playground as rust_playground,
    "spotify" => plugins_spotify as spotify,
    "string-utils" => plugins_string_utils as string_utils,
    "thingiverse" => plugins_thingiverse as thingiverse,
    "tiktok" => plugins_tiktok as tiktok,
    "titles" => plugins_titles as titles,
    "trustpilot" => plugins_trustpilot as trustpilot,
    "tvmaze" => plugins_tvmaze as tvmaze,
    "twitch" => plugins_twitch as twitch,
    "unwall" => plugins_unwall as unwall,
    "urban-dictionary" => plugins_urban_dictionary as urban_dictionary,
    "youtube" => plugins_youtube as youtube,
}

/// Registers every compiled-in bundled plugin that its configuration enables.
///
/// The host calls this once at startup, before the catalog is published. Plugins register
/// themselves through `inventory` when their `#[plugin]` implementation is linked in, so this
/// function only has to iterate the registrations — sorted by name for a deterministic catalog
/// and log order.
///
/// Configuration sections matching no bundled plugin are warned about as likely typos; enabled
/// sections for a bundled plugin whose cargo feature is disabled are reported together as
/// compiled out rather than silently ignored. Plugins that fail to initialize are logged and
/// skipped by the registry.
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
            zeta.plugin.name = %plugin,
            zeta.settings.key = %key,
            "unknown settings key (typo, or a setting removed in this version?)"
        );
    }

    // Sorted by name for a deterministic catalog and log order. Duplicate names are a build
    // error and are reported here regardless of whether the plugin is enabled; the registry
    // rejects them again for callers that bypass this function.
    let mut registrations: Vec<&'static zeta_plugin::PluginRegistration> =
        zeta_plugin::inventory::iter::<zeta_plugin::PluginRegistration>().collect();
    registrations.sort_unstable_by_key(|registration| registration.name);

    // A bundled plugin whose cargo feature is disabled has no registration, so an enabled
    // section for it would otherwise be parsed and silently ignored. Report them together once
    // instead of emitting one line per plugin (a minimal build of the repository config can
    // omit many).
    let registered: BTreeSet<&'static str> = registrations
        .iter()
        .map(|registration| registration.name)
        .collect();
    let compiled_out: Vec<&str> = plugins
        .sections
        .iter()
        .filter(|(name, section)| {
            section.enabled
                && BUNDLED_PLUGIN_NAMES.contains(&name.as_str())
                && !registered.contains(name.as_str())
        })
        .map(|(name, _)| name.as_str())
        .collect();

    if !compiled_out.is_empty() {
        tracing::warn!(
            plugins = ?compiled_out,
            "configured plugins are not compiled into this build"
        );
    }

    let mut previous: Option<&'static str> = None;
    for registration in registrations {
        if previous == Some(registration.name) {
            tracing::error!(
                zeta.plugin.name = registration.name,
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
                zeta.plugin.name = registration.name,
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

    /// The bundled plugin crates, derived from the `crates/plugins-*` directory names.
    fn plugin_crates() -> Vec<String> {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the facade lives under crates/");

        let mut plugins: Vec<String> = std::fs::read_dir(crates)
            .expect("read the crates directory")
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;

                // `plugins` is this facade and `plugins-shared` is a helper library, not a
                // bundled plugin.
                let plugin = name.strip_prefix("plugins-")?;

                (plugin != "shared").then(|| plugin.to_owned())
            })
            .collect();

        plugins.sort();

        plugins
    }

    /// Every `plugins-*` crate must be wired into the facade everywhere Cargo cannot derive it.
    ///
    /// Adding a plugin touches seven places across two files; a missed entry either fails the
    /// build (the dependency, the re-export) or silently ships a plugin that is never compiled
    /// (the `all` list, the TLS lists) or never named (the bundled-name list). This test turns
    /// the silent cases into a failure.
    #[test]
    fn every_plugin_crate_is_wired_into_the_facade() {
        let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("read the facade manifest");
        let lib = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
            .expect("read the facade source");

        for plugin in plugin_crates() {
            let snake = plugin.replace('-', "_");

            assert!(
                manifest.contains(&format!(
                    "plugins-{plugin} = {{ path = \"../plugins-{plugin}\", default-features = \
                     false, optional = true }}"
                )),
                "`{plugin}` is missing its optional dependency in crates/plugins/Cargo.toml"
            );
            assert!(
                manifest.contains(&format!("dep:plugins-{plugin}")),
                "`{plugin}` has no cargo feature enabling its dependency"
            );
            assert!(
                manifest.contains(&format!("\"{plugin}\",")),
                "`{plugin}` is missing from the `all` feature list"
            );

            for tls in ["native-tls", "native-tls-vendored", "rustls"] {
                assert!(
                    manifest.contains(&format!("\"plugins-{plugin}?/{tls}\"")),
                    "`{plugin}` is missing from the `{tls}` feature list"
                );
            }

            assert!(
                lib.contains(&format!("\"{plugin}\" => plugins_{snake} as {snake},")),
                "`{plugin}` is missing from the `bundled_plugins!` list"
            );
        }
    }

    /// Every bundled plugin self-registers through the `#[plugin]` macro, independently of any
    /// central list. `--all-features` compiles every one of them.
    #[cfg(feature = "all")]
    #[test]
    fn all_bundled_plugins_register_themselves() {
        assert_eq!(
            zeta_plugin::inventory::iter::<zeta_plugin::PluginRegistration>().count(),
            BUNDLED_PLUGIN_NAMES.len()
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
