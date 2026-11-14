//! The bundled plugins for the zeta IRC bot.
//!
//! Every plugin lives in its own `plugins-<name>` crate and implements
//! [`Plugin`](zeta_plugin::Plugin) from `zeta-plugin` through the `#[plugin]` attribute macro.
//! This crate is the facade over those crates: its cargo features select which plugin crates are
//! compiled in, preserving the feature names the host has always used.
//!
//! The `bundled_plugins!` list below is the single source of truth for that set. It generates:
//!
//! - one re-export per plugin crate, so the crate is linked;
//! - the [`BundledPlugin`] enum, with one variant per compiled-in plugin and every arm gated on
//!   the same cargo feature as the crate;
//! - the static construction contract ([`PluginSet`](zeta_plugin::PluginSet)): name lookup,
//!   settings validation and construction to a `LoadedPlugin<BundledPlugin>`;
//! - [`BundledPlugin::NAMES`], the compiled-in names the configuration layer and the registration
//!   loop iterate;
//! - `BUNDLED_PLUGIN_NAMES`, the names of every bundled plugin whether or not it is compiled in,
//!   so a configuration section for a compiled-out plugin is reported as such instead of as a
//!   typo.
//!
//! The runtime instantiates [`PluginRuntime`](zeta_plugin_runtime::PluginRuntime) with that enum,
//! so no plugin or handler future is boxed on the dispatch path.
//!
//! # Features
//!
//! `all` (the default) enables every bundled plugin. Each plugin also has a feature named after
//! it (`titles`, `youtube`, `unwall`, ...); the shared `database` and `http` features group the
//! capabilities the plugin crates need. See `Cargo.toml` for the exact graph.
//!
//! # Migrations
//!
//! The `database` feature also embeds `migrations/` and exposes the result through [`migrator`],
//! so the schema and the plugins that use it are owned by the same crate and the host never
//! reaches into this crate's source tree to find the migration files.

pub mod config;

pub use config::{PluginSection, PluginsConfig};

mod macros;

use macros::{
    define_bundled_plugin_enum, for_each_bundled_plugin, impl_plugin_for_bundled_plugin,
    impl_plugin_set_for_bundled_plugin, reexport_bundled_plugins,
};

// The single bundled-plugin list, expanded once per generated item. See `macros.rs`.
for_each_bundled_plugin!(reexport_bundled_plugins);
for_each_bundled_plugin!(define_bundled_plugin_enum);
for_each_bundled_plugin!(impl_plugin_for_bundled_plugin);
for_each_bundled_plugin!(impl_plugin_set_for_bundled_plugin);

/// The bundled plugins' database migrations, embedded at compile time.
///
/// The path passed to [`sqlx::migrate!`] is relative to this crate's manifest, so the migrations
/// stay next to the plugins that own them. The host applies them once at startup.
#[cfg(feature = "database")]
#[must_use]
pub fn migrator() -> &'static sqlx::migrate::Migrator {
    static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

    &MIGRATOR
}

/// Registers every compiled-in bundled plugin that its configuration enables.
///
/// The host calls this once at startup, before the catalog is published. The compiled-in plugin
/// set is [`BundledPlugin::NAMES`] — one entry per generated variant, in declaration order for a
/// deterministic catalog and log order — so registration is a plain loop over static data.
///
/// Configuration sections matching no bundled plugin are warned about as likely typos; enabled
/// sections for a bundled plugin whose cargo feature is disabled are reported together as
/// compiled out rather than silently ignored. Plugins that fail to initialize are logged and
/// skipped by the registry.
pub fn register_bundled_plugins(
    registry: &mut zeta_plugin_runtime::Registry<BundledPlugin>,
    ctx: &zeta_plugin::Context,
    plugins: &PluginsConfig,
) {
    for name in &plugins.unknown {
        tracing::warn!(
            zeta.plugin.section = %name,
            "unknown plugin configuration section (typo?)"
        );
    }

    for (plugin, key) in &plugins.unknown_setting_keys {
        tracing::warn!(
            zeta.plugin.name = %plugin,
            zeta.settings.key = %key,
            "unknown settings key (typo, or a setting removed in this version?)"
        );
    }

    // A bundled plugin whose cargo feature is disabled has no variant, so an enabled section for
    // it would otherwise be parsed and silently ignored. Report them together once instead of
    // emitting one line per plugin (a minimal build of the repository config can omit many).
    let compiled_out: Vec<&str> = plugins
        .sections
        .iter()
        .filter(|(name, section)| {
            section.enabled
                && BUNDLED_PLUGIN_NAMES.contains(&name.as_str())
                && !BundledPlugin::NAMES.contains(&name.as_str())
        })
        .map(|(name, _)| name.as_str())
        .collect();

    if !compiled_out.is_empty() {
        tracing::warn!(
            zeta.plugin.compiled_out.names = ?compiled_out,
            "configured plugins are not compiled into this build"
        );
    }

    for name in BundledPlugin::NAMES {
        let (enabled, section) = plugins
            .sections
            .get(*name)
            .map_or((plugins.enabled_by_default, None), |section| {
                (section.enabled, Some(&section.settings))
            });

        if enabled {
            registry.register(ctx, name, section);
        } else {
            tracing::info!(
                zeta.plugin.name = %name,
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
    use crate::{BUNDLED_PLUGIN_NAMES, BundledPlugin};

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
    /// Adding a plugin touches four places across two files; a missed entry either fails the
    /// build (the dependency, the re-export) or silently ships a plugin that is never compiled
    /// (the `all` list) or never named (the bundled-name list). This test turns the silent cases
    /// into a failure.
    #[test]
    fn every_plugin_crate_is_wired_into_the_facade() {
        let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("read the facade manifest");
        let list = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/macros.rs"))
            .expect("read the facade macros");
        // Normalize whitespace so the list's column alignment does not affect the check.
        let list = list.split_whitespace().collect::<Vec<_>>().join(" ");

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

            assert!(
                list.contains(&format!("\"{plugin}\" => plugins_{snake}::")),
                "`{plugin}` is missing from the `for_each_bundled_plugin!` list"
            );
        }
    }

    /// Every bundled plugin has a generated variant when every plugin is compiled in, and the
    /// variant's name matches the bundled-name list the configuration layer reports against.
    #[cfg(feature = "all")]
    #[test]
    fn every_bundled_plugin_has_a_variant() {
        let generated: BTreeSet<&str> = BundledPlugin::NAMES.iter().copied().collect();
        let bundled: BTreeSet<&str> = BUNDLED_PLUGIN_NAMES.iter().copied().collect();

        assert_eq!(generated, bundled);
        assert!(
            BundledPlugin::NAMES
                .windows(2)
                .all(|pair| pair[0] <= pair[1]),
            "the generated names must stay sorted for a deterministic catalog and log order: {:?}",
            BundledPlugin::NAMES
        );
    }

    /// The generated lookup finds a compiled-in plugin and reports an unknown name as absent.
    #[cfg(feature = "all")]
    #[tokio::test]
    async fn runtime_plugin_lookup_matches_the_generated_variants() {
        use zeta_plugin::PluginSet;

        let ctx = zeta_test_support::test_context();

        let loaded = BundledPlugin::construct(&ctx, "help", None)
            .expect("the help plugin should construct")
            .expect("`help` should have a BundledPlugin variant");

        assert_eq!(loaded.name, "help");
        assert_eq!(loaded.plugin.name(), "help");

        assert!(
            BundledPlugin::construct(&ctx, "not_a_plugin", None)
                .expect("an unknown name is not an error")
                .is_none()
        );
        assert!(BundledPlugin::validate("not_a_plugin", None).is_none());
    }
}
