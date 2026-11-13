//! A client and parser for Kagi Search.
//!
//! This crate provides an asynchronous [`Client`] for querying [Kagi](https://kagi.com) with a
//! session token and parsing the streamed responses into [`SearchResult`]s and [`ImageResult`]s.
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
//! To get started, add this crate to your `Cargo.toml`. The main entry points for searching are
//! the [`Client::search`] and [`Client::images`] methods.
//!
//! ```rust,no_run
//! use kagi::{Client, Error};
//!
//! # async fn example() -> Result<(), Error> {
//! // Create a new client with a Kagi session token.
//! let client = Client::try_new("kagi session token")?;
//!
//! // Search for the given query.
//! let results = client.search("rust programming language").await?;
//!
//! if let Some(result) = results.first() {
//!     println!("{} - {}", result.title, result.url);
//! }
//!
//! // Or search for images.
//! let images = client.images("ferrous wheel").await?;
//!
//! if let Some(image) = images.first() {
//!     println!("{} ({}x{}) {}", image.title, image.width, image.height, image.image_url);
//! }
//! # Ok(())
//! # }
//! ```

// Allow repetition of structure name instead of replacing with self as the output from
// rust-analyzer becomes more readable
use std::time::Duration;

mod client;
mod error;
mod model;
mod parse;

pub use client::Client;
pub use error::{Error, RequestError};
pub use model::{ImageResult, SearchResult};

/// Kagi base URL.
pub const BASE_URL: &str = "https://kagi.com";
/// A realistic browser user agent sent with requests, as Kagi serves an altered experience to
/// clients it identifies as non-browser.
///
/// Kept in step with the `zeta-http` default so the binary does not present two browser
/// generations to different services.
pub const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:157.0) Gecko/20100101 Firefox/157.0";
/// The duration before a HTTP request times out.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// The duration of a single session. Once this duration has passed, a new session will be created.
pub const SESSION_DURATION: Duration = Duration::from_mins(15);
/// The default `Accept-Language` header sent with requests.
pub const LANGUAGE: &str = "en-US,en;q=0.9";

/// Options for configuring a Kagi [`Client`].
///
/// Unset options keep the crate's defaults: [`HTTP_TIMEOUT`] for the timeout, [`USER_AGENT`] for
/// the user agent, [`SESSION_DURATION`] for the session duration, and [`LANGUAGE`] for the
/// `Accept-Language` header.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ClientOptions {
    /// The duration before an HTTP request times out.
    ///
    /// Defaults to [`HTTP_TIMEOUT`] when unset.
    pub timeout: Option<Duration>,
    /// The `User-Agent` header sent with requests.
    ///
    /// Defaults to [`USER_AGENT`] when unset.
    pub user_agent: Option<String>,
    /// The duration of a single session.
    ///
    /// Defaults to [`SESSION_DURATION`] when unset.
    pub session_duration: Option<Duration>,
    /// The `Accept-Language` header sent with requests.
    ///
    /// Defaults to [`LANGUAGE`] when unset.
    pub language: Option<String>,
}
