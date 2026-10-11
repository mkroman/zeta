//! HTTP client infrastructure for the zeta IRC bot and its plugins.
//!
//! Two independent clients live here: the plain `reqwest` client shared by the API plugins
//! (behind the `http` feature) and the browser-emulated `wreq` client for anti-bot-protected
//! sites (behind the `emulated` feature), together with the OAuth2 client-credentials token
//! cache and the transport-error wrappers that keep credentials out of formatted output.

#![allow(clippy::use_self)]

pub mod http;
/// OAuth2 client-credentials token caching.
#[cfg(feature = "oauth")]
pub mod oauth;

mod error;

#[cfg(any(feature = "request", feature = "http"))]
pub use error::RequestError;
#[cfg(feature = "emulated")]
pub use error::WreqError;
