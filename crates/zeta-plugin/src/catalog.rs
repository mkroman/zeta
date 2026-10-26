//! Metadata about the plugins registered with the bot.

use crate::CommandSpec;

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
/// The catalog is published to the context's shared state once registration is finished, so
/// plugins can look it up through [`Context::shared`](crate::Context::shared) to discover
/// other plugins.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PluginCatalog {
    /// The registered plugins, in registration order.
    pub entries: Vec<CatalogEntry>,
}
