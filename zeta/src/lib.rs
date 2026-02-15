//! Zeta is an opinionated IRC bot with a bunch of plugins.

#![allow(clippy::use_self)]

pub mod command;
/// Configuration loading and validation
pub mod config;
/// Commonly used constants
pub mod consts;
/// Shared context for plugins
pub mod context;
/// Database integration
pub mod database;
/// DNS resolution
pub mod dns;
mod error;
mod http;
mod plugin;
mod utils;
mod zeta;

pub use config::Config;
pub use error::Error;
pub use plugin::{Plugin, Registry};
pub use zeta::Zeta;
