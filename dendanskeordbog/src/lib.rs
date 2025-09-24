// Allow repetition of structure name instead of replacing with self as the output from
// rust-analyzer becomes more readable
#![allow(clippy::use_self)]

#[cfg(feature = "client")]
pub mod client;
mod error;
pub mod types;

#[cfg(feature = "client")]
pub use client::Client;
pub use error::Error;
pub use types::DictionaryDocument;
