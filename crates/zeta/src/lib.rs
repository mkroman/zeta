//! Zeta is an opinionated IRC bot with a bunch of plugins.
//!
//! The bundled plugins live in the `plugins` crate; the plugin API and the infrastructure they
//! share with this host live in `zeta-plugin`. This crate carries the IRC bot itself: the
//! client loop, event dispatching, configuration, and the plugin task machinery.

#![allow(clippy::use_self)]

/// Configuration loading and validation
pub mod config;
/// Commonly used constants
pub mod consts;
/// Database integration
#[cfg(feature = "database")]
pub mod database;
mod error;
/// Plugin host support: the registry facade, event dispatching, and plugin tasks
pub mod plugin;
mod zeta;

/// Single-slot time-to-live caching
pub use zeta_cache;
/// HTTP client integration.
///
/// Enabled by the `http` feature (plain reqwest) or the `emulated` feature (browser-emulated
/// wreq for anti-bot-protected sites).
#[cfg(any(feature = "http", feature = "emulated", feature = "mirror"))]
pub use zeta_http as http;
/// Shared media mirroring
#[cfg(feature = "mirror")]
pub use zeta_mirror as mirror;
/// URL helper utillities
pub use zeta_url as url;
/// Duration formatting and parsing utilities
pub use zeta_utils::duration;

pub use config::Config;
pub use error::Error;
#[cfg(any(feature = "http", feature = "mirror"))]
pub use error::RequestError;
#[cfg(feature = "emulated")]
pub use error::WreqError;
pub use plugin::{
    CatalogEntry, ErasedPlugin, Plugin, PluginCatalog, PluginsConfig, RegisteredPlugin, Registry,
};
pub use zeta::Zeta;
