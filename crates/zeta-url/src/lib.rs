//! URL helpers for the zeta IRC bot.
//!
//! A leaf crate on purpose: the standalone API client crates share the redaction helpers with
//! the host, and the dependency direction runs the other way. Three concerns live here, one
//! module each: extracting URLs from message text ([`ExtractUrls`]), the path-segment validators
//! the per-plugin URL matchers build on, and [`RedactedUrl`], the loggable form of a URL that
//! keeps credential-bearing query strings out of logs.
//!
//! The modules are implementation details; everything is re-exported at the crate root, so call
//! sites keep naming the item directly, e.g. `zeta_url::ExtractUrls`.

mod extract;
mod path;
mod redact;

pub use extract::{ExtractUrls, ExtractedUrl, HTTP_SCHEMES, SchemeMap};
pub use path::{
    is_identifier, is_numeric_segment, is_prefixed_numeric_id, last_path_segment, path_segments,
    query_param, strip_www,
};
pub use redact::RedactedUrl;
