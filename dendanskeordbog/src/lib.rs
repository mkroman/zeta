#[cfg(feature = "client")]
pub mod client;
mod error;
pub mod types;

#[cfg(feature = "client")]
pub use client::Client;
pub use error::Error;
pub use types::DictionaryDocument;
