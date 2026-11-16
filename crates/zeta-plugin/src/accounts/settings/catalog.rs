//! The snapshot of the per-user settings the loaded plugins declared.
//!
//! The runtime builds the catalog from the registered plugins'
//! [`Registered::USER_SETTINGS`](crate::Registered::USER_SETTINGS) once registration finishes
//! and publishes it next to the [`PluginCatalog`](crate::PluginCatalog), so the `.settings`
//! command lists exactly the settings of the plugins that are actually loaded.

use tracing::warn;

use super::spec::UserSettingSpec;

/// The per-user settings the registered plugins declared, sorted by plugin and key.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserSettingsCatalog {
    /// Every registered descriptor, sorted by plugin and then key.
    specs: Vec<&'static UserSettingSpec>,
}

impl UserSettingsCatalog {
    /// Creates a catalog from the registered plugins' descriptor slices.
    ///
    /// A plugin that spells a different `plugin` name in its descriptors than the one that
    /// carries them produces a warning naming both — the mismatch is a programming error, not a
    /// configuration one, so it never fails startup.
    #[must_use]
    pub fn new(
        slices: impl IntoIterator<Item = (&'static str, &'static [UserSettingSpec])>,
    ) -> Self {
        let mut warnings = Vec::new();
        let mut specs: Vec<&'static UserSettingSpec> = slices
            .into_iter()
            .flat_map(|(plugin, specs)| {
                for spec in specs {
                    if spec.plugin != plugin {
                        warnings.push((plugin, spec.plugin, spec.key));
                    }
                }

                specs.iter().collect::<Vec<&UserSettingSpec>>()
            })
            .collect();

        for (plugin, spec_plugin, key) in warnings {
            warn!(
                zeta.plugin.name = plugin,
                "plugin declares its `{key}` user setting under the name `{spec_plugin}`"
            );
        }

        specs.sort_by_key(|spec| (spec.plugin, spec.key));

        Self { specs }
    }

    /// Every registered descriptor, sorted by plugin and then key.
    #[must_use]
    pub fn all(&self) -> &[&'static UserSettingSpec] {
        &self.specs
    }

    /// The descriptors one plugin declared, in the catalog's order.
    pub fn for_plugin<'a>(
        &'a self,
        plugin: &'a str,
    ) -> impl Iterator<Item = &'static UserSettingSpec> + 'a {
        self.specs
            .iter()
            .copied()
            .filter(move |spec| spec.plugin == plugin)
    }

    /// The descriptor addressed as `plugin.key`, if it is registered.
    #[must_use]
    pub fn find(&self, plugin: &str, key: &str) -> Option<&'static UserSettingSpec> {
        self.specs
            .iter()
            .copied()
            .find(|spec| spec.plugin == plugin && spec.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::super::spec::{UserSettingKind, UserSettingSpec};

    use super::*;

    static ALPHA_MODE: UserSettingSpec = UserSettingSpec {
        plugin: "alpha",
        key: "mode",
        kind: UserSettingKind::Enum(&["on", "off"]),
        description: "the alpha mode",
        default: Some("on"),
    };
    static ALPHA_ACCENT: UserSettingSpec = UserSettingSpec {
        plugin: "alpha",
        key: "accent",
        kind: UserSettingKind::Enum(&["DA", "DE"]),
        description: "the alpha accent",
        default: None,
    };
    static GAMMA_MODE: UserSettingSpec = UserSettingSpec {
        plugin: "gamma",
        key: "mode",
        kind: UserSettingKind::Enum(&["summary", "takeaway"]),
        description: "the gamma mode",
        default: Some("summary"),
    };

    // `new` takes `&'static [UserSettingSpec]` slices, so the test slices are statics.
    static ALPHA_SPECS: [UserSettingSpec; 2] = [ALPHA_MODE, ALPHA_ACCENT];
    static GAMMA_SPECS: [UserSettingSpec; 1] = [GAMMA_MODE];

    #[test]
    fn sorts_by_plugin_then_key_and_finds_by_address() {
        let catalog =
            UserSettingsCatalog::new([("gamma", &GAMMA_SPECS[..]), ("alpha", &ALPHA_SPECS[..])]);

        let ids: Vec<String> = catalog.all().iter().map(|spec| spec.id()).collect();
        assert_eq!(
            ids,
            ["alpha.accent", "alpha.mode", "gamma.mode"],
            "the catalog sorts before the `.settings` listing renders it"
        );

        assert_eq!(
            catalog.find("alpha", "mode").map(UserSettingSpec::id),
            Some("alpha.mode".to_owned())
        );
        assert!(catalog.find("alpha", "missing").is_none());
        assert!(catalog.find("missing", "mode").is_none());
    }

    #[test]
    fn for_plugin_scopes_the_descriptors() {
        let catalog =
            UserSettingsCatalog::new([("alpha", &ALPHA_SPECS[..]), ("gamma", &GAMMA_SPECS[..])]);

        let keys: Vec<&str> = catalog.for_plugin("alpha").map(|spec| spec.key).collect();
        assert_eq!(keys, ["accent", "mode"]);
        assert_eq!(catalog.for_plugin("missing").count(), 0);
    }

    #[test]
    fn a_mismatched_plugin_name_is_carried_and_warned() {
        // The catalog carries the descriptor even when the name disagrees; the warning is the
        // operator's cue to fix the slice.
        static MISNAMED: UserSettingSpec = UserSettingSpec {
            plugin: "gamma",
            key: "accent",
            kind: UserSettingKind::Enum(&["a", "b"]),
            description: "declared under the wrong plugin name",
            default: None,
        };
        static MISNAMED_SPECS: [UserSettingSpec; 1] = [MISNAMED];

        let catalog = UserSettingsCatalog::new([("alpha", &MISNAMED_SPECS[..])]);

        assert_eq!(catalog.all().len(), 1);
        assert_eq!(catalog.for_plugin("gamma").count(), 1);
    }
}
