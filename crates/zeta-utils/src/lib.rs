//! Shared utility helpers for the zeta IRC bot and its plugins.
//!
//! Small, dependency-light building blocks: text shaping, joining, and matching ([`text`]),
//! string setting resolution ([`settings`]), formatting and parsing of durations and points in
//! time ([`duration`]), and error-chain rendering for logs ([`error`]).

pub mod duration;
pub mod error;
pub mod settings;
pub mod text;
