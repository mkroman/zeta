//! Code shared by several bundled plugins.
//!
//! A helper used by a single plugin stays in that plugin's own crate; this crate is the home
//! for the pieces more than one plugin needs, so sharing them never pulls in the plugin that
//! introduced them.

pub mod urls;

/// The IRC-facing reply flow shared by the mirroring plugins.
#[cfg(feature = "mirror")]
pub mod mirror;
