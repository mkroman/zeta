//! Runtime support for hosting zeta plugins.
//!
//! This crate owns the machinery that runs plugins: the [`Registry`] that constructs them from
//! their [`PluginRegistration`](zeta_plugin::PluginRegistration), the catalog published to the
//! shared state, and the [`PluginRuntime`] that spawns each plugin into its own task and routes
//! incoming IRC messages to the plugins that registered interest. The plugin-facing API itself
//! lives in `zeta-plugin`.

mod dispatch;
mod registry;
mod runtime;
mod task;

pub use registry::Registry;
pub use runtime::PluginRuntime;
