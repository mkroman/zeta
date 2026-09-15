//! A client and parser for Kagi Search.
//!
//! This crate provides an asynchronous [`Client`] for querying [Kagi](https://kagi.com) with a
//! session token and parsing the streamed HTML response into [`SearchResult`]s.
//!
//! ## Features
//!
//! - **`default-tls`** *(enabled by default)*: selects the default TLS backend (**`rustls`**).
//!   Disable default features and choose a TLS backend explicitly with **`rustls`**,
//!   **`native-tls`** or **`native-tls-vendored`**.
//! - **`gzip`**, **`brotli`**, **`deflate`**, **`zstd`** *(all enabled by default)*:
//!   decompression of compressed responses.
//!
//! ## Quick Start
//!
//! To get started, add this crate to your `Cargo.toml`. The main entry point for searching is the
//! [`Client::search`] method.
//!
//! ```rust,no_run
//! use kagi::{Client, Error};
//!
//! # async fn example() -> Result<(), Error> {
//! // Create a new client with a Kagi session token.
//! let client = Client::with_token("kagi session token");
//!
//! // Search for the given query.
//! let results = client.search("rust programming language").await?;
//!
//! if let Some(result) = results.first() {
//!     println!("{} - {}", result.title, result.url);
//! }
//! # Ok(())
//! # }
//! ```

// Allow repetition of structure name instead of replacing with self as the output from
// rust-analyzer becomes more readable
#![allow(clippy::use_self)]

use std::time::Duration;

mod client;
mod error;

pub use client::Client;
pub use error::Error;

/// Kagi base URL.
pub const BASE_URL: &str = "https://kagi.com";
/// A realistic browser user agent sent with requests, as Kagi serves an altered experience to
/// clients it identifies as non-browser.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:155.0) Gecko/20100101 Firefox/155.0";
/// The duration before a HTTP request times out.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// The duration of a single session. Once this duration has passed, a new session will be created.
pub const SESSION_DURATION: Duration = Duration::from_mins(15);

/// Represents a single search result obtained from the search operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchResult {
    /// The title of the search result.
    pub title: String,
    /// The URL of the search result.
    pub url: String,
    /// The description.
    pub description: String,
}
