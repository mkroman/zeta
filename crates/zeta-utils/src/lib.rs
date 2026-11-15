//! Shared utility helpers for the zeta IRC bot and its plugins.
//!
//! Small, dependency-light building blocks: text shaping, joining, and matching ([`text`]), IRC
//! reply formatting, addressing, and line budgets ([`irc`]), string setting resolution
//! ([`settings`]), formatting and parsing of durations and points in time ([`duration`]), and
//! error-chain rendering for logs ([`error`]).

pub mod duration;
pub mod error;
pub mod irc;
pub mod settings;
pub mod text;
