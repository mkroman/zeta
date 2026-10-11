//! Shared utility helpers for the zeta IRC bot and its plugins.
//!
//! Small, dependency-light building blocks: text shaping for IRC replies
//! ([`utils`]), formatting and parsing of durations and points in time
//! ([`duration`]), and poisoning-recovering lock helpers ([`sync`]).

pub mod duration;
pub mod sync;
pub mod utils;
