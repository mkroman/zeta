//! Zeta is an opinionated IRC bot with a bunch of plugins.

#![allow(clippy::use_self)]

pub mod command;
/// Configuration loading and validation
pub mod config;
/// Commonly used constants
pub mod consts;
/// Database integration
pub mod database;
mod dns;
mod error;
mod plugin;
mod utils;
mod zeta;

pub use config::Config;
pub use error::Error;
pub use plugin::{Plugin, Registry};
pub use zeta::Zeta;
