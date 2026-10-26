//! A client and parser for the Danish Dictionary (*Den Danske Ordbog*).
//!
//! This crate provides tools to query the official Danish dictionary web service at `ordnet.dk`
//! and parse the resulting HTML into structured Rust types.
//!
//! ## Features
//!
//! - **`client`**: Enables the `async` [`Client`] for making live requests to the dictionary
//!   service. (Enabled by default).
//! - **`serde`**: Adds `Serialize` and `Deserialize` derives on all public data structures in the
//!   [`types`] module.
//!
//! ## Quick Start
//!
//! To get started, add this crate to your `Cargo.toml`. The main entry point for fetching data is
//! the [`Client::query`] method.
//!
//! ```rust,no_run
//! use dendanskeordbog::{Client, Error};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Error> {
//!     // Create a new client with default settings.
//!     let client = Client::new();
//!
//!     // Query the dictionary for the word "hest".
//!     let document = client.query("hest").await?;
//!
//!     // A document can contain multiple entries (e.g., for homonyms).
//!     if let Some(entry) = document.entries.first() {
//!         println!("Found entry for: {}", entry.head.keyword);
//!         println!("Part of speech: {}", entry.pos);
//!
//!         if let Some(definition) = entry.definitions.first() {
//!             println!("First definition: {}", definition.description);
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```

/// Contains the asynchronous client for querying the dictionary service.
#[cfg(feature = "client")]
pub mod client;

/// Contains the crate's error types.
mod error;

/// Contains the parsing primitives shared by the dictionary types.
mod parse;

/// Contains all structured data types representing dictionary content.
pub mod types;

/// Re-export of the asynchronous `Client` for convenient access.
#[cfg(feature = "client")]
pub use client::Client;
/// Re-export of the primary `Error` enum for convenient access.
pub use error::Error;
/// Re-export of the redacted request error type for convenient access.
#[cfg(feature = "client")]
pub use error::RequestError;
/// Re-export of the shared HTML parsing primitives.
pub use parse::{FromHtml, Selectors};
/// Re-export of the top-level `DictionaryDocument` struct.
pub use types::DictionaryDocument;
