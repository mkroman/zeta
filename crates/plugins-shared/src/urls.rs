//! URL matchers shared by multiple plugins.
//!
//! A matcher used by a single plugin stays in that plugin's `<name>/urls.rs`; this module is
//! the home for the ones more than one plugin needs. It is compiled when any of those plugins
//! is enabled, so sharing a matcher never pulls in the plugin that introduced it.

pub mod youtube;
