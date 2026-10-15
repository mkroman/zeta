//! Plugin host support: event dispatching to the plugins.
//!
//! The plugin API itself — the [Plugin trait](zeta_plugin::Plugin), the events, the
//! [context](zeta_plugin::Context), the [registry](zeta_plugin::Registry) and the
//! [plugin task](zeta_plugin::PluginTask) that runs one plugin — lives in `zeta-plugin`; the
//! bundled plugins live in the `plugins` crate. This module carries what remains host-side: the
//! dispatch index that routes incoming IRC messages to the plugins that registered interest.

pub mod dispatch;

pub use plugins::PluginsConfig;
pub use zeta_plugin::{
    CatalogEntry, Context, ErasedPlugin, Filters, Plugin, PluginCatalog, PluginTask,
    RegisteredPlugin, Registry, SharedState,
};
