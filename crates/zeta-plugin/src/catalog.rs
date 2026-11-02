//! Metadata about the plugins registered with the bot.

use crate::{CommandSpec, UrlScope};

/// Metadata about a registered plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct CatalogEntry {
    /// The name of the plugin.
    pub name: String,
    /// The commands handled by the plugin.
    pub commands: Vec<CommandSpec>,
    /// The scope of URLs the plugin registered interest in.
    pub url_scope: UrlScope,
}

impl CatalogEntry {
    /// Creates a new catalog entry.
    #[must_use]
    pub const fn new(name: String, commands: Vec<CommandSpec>, url_scope: UrlScope) -> Self {
        Self {
            name,
            commands,
            url_scope,
        }
    }

    /// Whether the plugin declares that it handles links posted on `host`.
    ///
    /// Only exact host registrations count: a plugin registered for every URL
    /// ([`UrlScope::Any`]) advertises no specific hosts and is therefore not a reason for a
    /// generic handler to skip a link.
    #[must_use]
    pub fn handles_host(&self, host: &str) -> bool {
        match self.url_scope {
            UrlScope::Hosts(hosts) => hosts
                .iter()
                .any(|handled| handled.eq_ignore_ascii_case(host)),
            UrlScope::None | UrlScope::Any => false,
        }
    }
}

/// Snapshot of the plugins registered with the bot and the events they handle.
///
/// The catalog is published to the context's shared state once registration is finished, so
/// plugins can look it up through [`Context::shared`](crate::Context::shared) to discover
/// other plugins. It is not replaced afterwards, so consumers may cache the handle.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct PluginCatalog {
    /// The registered plugins, in registration order.
    pub entries: Vec<CatalogEntry>,
}

impl PluginCatalog {
    /// Creates a new catalog from `entries`, in registration order.
    #[must_use]
    pub const fn new(entries: Vec<CatalogEntry>) -> Self {
        Self { entries }
    }
}
