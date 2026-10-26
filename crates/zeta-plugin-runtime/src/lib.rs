//! Runtime support for hosting zeta plugins.
//!
//! This crate owns the machinery that runs plugins: the registry that constructs them from
//! their [`PluginRegistration`](zeta_plugin::PluginRegistration), the catalog published to the
//! shared state, the long-lived [`PluginTask`] each plugin runs in, and the [`EventIndex`] that
//! routes incoming IRC messages to the plugins that registered interest. The plugin-facing API
//! itself lives in `zeta-plugin`.

mod dispatch;
mod registry;
mod task;

pub use dispatch::EventIndex;
pub use registry::{RegisteredPlugin, Registry};
pub use task::PluginTask;
pub use zeta_plugin::{CatalogEntry, PluginCatalog};
