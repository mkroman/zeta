//! Runtime support for hosting zeta plugins.
//!
//! This crate owns the machinery that runs plugins: the [`Registry`] that constructs the
//! compiled-in plugins from a [`PluginSet`](zeta_plugin::PluginSet), the catalog published to
//! the shared state, and the [`PluginRuntime`] that spawns each plugin into its own task and
//! routes incoming IRC messages to the plugins that registered interest. The plugin-facing API
//! itself lives in `zeta-plugin`.
//!
//! The runtime is generic over the plugin type and never boxes a plugin or its handler future:
//! the facade's generated enum is the only plugin type the host instantiates it with, so each
//! handler call is a static dispatch awaited inline on the plugin's task.

mod dispatch;
mod registry;
mod runtime;
mod task;

// Test-facing internals: the dispatch span test lives in its own test binary and drives the
// index directly (`tests/dispatch_spans.rs`). Hidden from the documentation and not a stability
// promise — the supported API is `PluginRuntime`.
#[doc(hidden)]
pub use dispatch::{Envelope, EventIndex};
pub use registry::Registry;
pub use runtime::PluginRuntime;
