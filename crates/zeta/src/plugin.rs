//! Plugin host support: event dispatching to the plugins.
//!
//! The plugin API itself — the [Plugin trait](zeta_plugin::Plugin), the events, the
//! [context](zeta_plugin::Context), the [registry](zeta_plugin::PluginRegistry) and the
//! [plugin task](zeta_plugin::PluginTask) that runs one plugin — lives in `zeta-plugin`; the
//! bundled plugins live in the `plugins` crate. This module carries what remains host-side: the
//! event dispatcher that routes incoming IRC messages to the plugins that registered interest.

pub mod dispatch;

pub use dispatch::EventDispatcher;
pub use plugins::PluginsConfig;
pub use zeta_plugin::{
    Context, ErasedPlugin, FilterGate, Plugin, PluginCatalog, PluginEntry, PluginRegistry,
    PluginTask, RegisteredPlugin, SharedState,
};
