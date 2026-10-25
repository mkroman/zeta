//! The bundled plugins for the zeta IRC bot.
//!
//! Every plugin implements [`Plugin`](zeta_plugin::Plugin) from `zeta-plugin` through the
//! `#[plugin]` attribute macro, which also registers the plugin with the host through
//! `inventory`. [`register_bundled_plugins`] discovers the compiled-in plugins from those
//! registrations, resolves each one's section in [`PluginsConfig`], and registers the enabled
//! ones with the [`Registry`](zeta_plugin::Registry).

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

pub use config::{PluginConfig, PluginSection, PluginsConfig};

/// The names of every bundled plugin, whether or not its cargo feature is enabled.
///
/// Configuration sections for these names describe a plugin that was compiled out, not a typo,
/// so the host can report the two cases differently. Kept in sync with the module declarations
/// by the `bundled_plugin_names_match_the_declared_modules` test.
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
    registry: &mut zeta_plugin::Registry,
    ctx: &zeta_plugin::Context,
    plugins: &PluginsConfig,
) {
    for name in &plugins.unknown {
        tracing::warn!(section = %name, "unknown plugin configuration section (typo?)");
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

// The bundled plugin modules are declared literally so rustfmt can discover them. Rustfmt does
// not expand macros, so a generated `pub mod` would leave every plugin file invisible to
// `cargo fmt`; the plugins register themselves through their `#[plugin]` implementations.
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

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

    /// The plugin name declared by a `#[plugin(name = "...")]` attribute, if present.
    fn plugin_attribute_name(attributes: &[syn::Attribute]) -> Option<String> {
        attributes.iter().find_map(|attribute| {
            if !attribute.path().is_ident("plugin") {
                return None;
            }

            let arguments = attribute
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .ok()?;

            arguments.into_iter().find_map(|argument| {
                let syn::Meta::NameValue(name_value) = argument else {
                    return None;
                };
                if !name_value.path.is_ident("name") {
                    return None;
                }
                let syn::Expr::Lit(expression) = name_value.value else {
                    return None;
                };
                let syn::Lit::Str(name) = expression.lit else {
                    return None;
                };

                Some(name.value())
            })
        })
    }

    /// Every plugin module carries exactly one `#[plugin]` impl, named after the module. The
    /// registration the macro generates from that name is what the host discovers, so a typo
    /// would silently register the plugin under the wrong configuration section.
    #[test]
    fn plugin_modules_declare_one_matching_plugin_impl() {
        let source = include_str!("lib.rs");
        let modules = plugin_modules(source);
        assert_eq!(modules.len(), 35);

        for name in modules {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join(format!("{name}.rs"));
            let source = std::fs::read_to_string(&path).expect("read plugin source");
            let file = syn::parse_file(&source).expect("a parsable plugin source file");

            let declared: Vec<String> = file
                .items
                .into_iter()
                .filter_map(|item| {
                    let syn::Item::Impl(item) = item else {
                        return None;
                    };
                    plugin_attribute_name(&item.attrs)
                })
                .collect();

            assert_eq!(
                declared,
                [name],
                "the module and its `#[plugin(name = ...)]` attribute must match"
            );
        }
    }

    /// The warning list of bundled names must cover every module, including plugins whose
    /// cargo feature is disabled in this build.
    #[test]
    fn bundled_plugin_names_match_the_declared_modules() {
        let modules = plugin_modules(include_str!("lib.rs"));
        let declared: BTreeSet<&str> = modules.iter().map(String::as_str).collect();
        let bundled: BTreeSet<&str> = crate::BUNDLED_PLUGIN_NAMES.iter().copied().collect();

        assert_eq!(bundled, declared);
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
