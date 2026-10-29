//! A client and parser for the Reddit API.
//!
//! This crate provides an asynchronous [`Client`] for querying the Reddit API with OAuth2
//! client-credentials, classifies reddit.com and redd.it URLs into [`Link`]s, and parses
//! listing responses into [`Submission`], [`Subreddit`] and [`Comment`] models.
//!
//! Reddit HTML-escapes plain-text fields unless a request opts into raw JSON; the models decode
//! them on deserialization, so their text fields hold the visible text. URL fields and
//! [`Comment::body_html`] are kept as the API sends them.
//!
//! ## Features
//!
//! - **`default-tls`** *(enabled by default)*: selects the default TLS backend (**`rustls`**).
//!   Disable default features and choose a TLS backend explicitly with **`rustls`**,
//!   **`native-tls`** or **`native-tls-vendored`**.

use std::time::Duration;

mod client;
mod error;
mod model;
mod urls;

pub use client::{Client, ClientOptions};
pub use error::{Error, RequestError};
pub use model::{Comment, Item, Link, Listing, Media, RedditVideo, Submission, Subreddit};
pub use urls::{URL_HOSTS, classify_reddit_url};

/// Reddit API base URL.
pub const BASE_URL: &str = "https://www.reddit.com";
/// Reddit OAuth API base URL, for authenticated requests.
pub const OAUTH_BASE_URL: &str = "https://oauth.reddit.com";
/// The OAuth2 client-credentials token endpoint.
pub const TOKEN_URL: &str = "https://www.reddit.com/api/v1/access_token";
/// Identifying HTTP user agent for API requests (i.e. `linux:zeta:<VERSION> (by /u/drizz)`)
pub const USER_AGENT: &str = concat!("rust:reddit:", env!("CARGO_PKG_VERSION"), " (by /u/drizz)");
/// The duration before a HTTP request times out.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// The maximum number of characters of an unparseable response body that reaches the logs.
const LOGGED_BODY_LENGTH: usize = 512;
