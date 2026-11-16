//! A single-slot cache whose entries expire after a time-to-live.
//!
//! Shared by plugins that cache an API response (e.g. currency lists or video categories) and
//! refresh it lazily after a configured TTL. Reads do not need to await, matching the pattern
//! of command handlers that peek at the cache synchronously while refreshes happen in `async`
//! code.
//!
//! Also provides [`TtlMap`], a keyed variant whose entries expire individually.
//!
//! The caches live in `ttl_cache` and `ttl_map` and share the expiry bookkeeping in `entry`;
//! every public type is re-exported at the crate root, so call sites keep naming
//! `zeta_cache::TtlCache` and `zeta_cache::TtlMap`.

mod entry;
mod ttl_cache;
mod ttl_map;

pub use ttl_cache::TtlCache;
pub use ttl_map::TtlMap;
