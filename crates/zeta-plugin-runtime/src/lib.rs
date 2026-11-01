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

// Test-facing internals: the dispatch span test lives in its own test binary and drives the
// index directly (`tests/dispatch_spans.rs`). Hidden from the documentation and not a stability
// promise — the supported API is `PluginRuntime`.
#[doc(hidden)]
pub use dispatch::EventIndex;
pub use registry::Registry;
pub use runtime::PluginRuntime;
