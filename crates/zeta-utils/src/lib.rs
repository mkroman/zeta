//! Shared utility helpers for the zeta IRC bot and its plugins.
//!
//! Small, dependency-light building blocks: text shaping and matching ([`text`]), IRC reply
//! formatting, addressing, and line budgets ([`irc`]), string setting resolution
//! ([`settings`]), formatting and parsing of durations and points in time ([`duration`]), and
//! poisoning-recovering lock helpers ([`sync`]).

pub mod duration;
pub mod irc;
pub mod settings;
pub mod sync;
pub mod text;
