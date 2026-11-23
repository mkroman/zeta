//! The hostmasks each identity has been seen from.

use sqlx::types::chrono::{DateTime, Utc};

/// One hostmask an identity has been observed from.
///
/// Accounts are stable across connections, so the hosts an account has been used from stay
/// auditable: `first_seen` never moves, `last_seen` moves on every observation.
#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct ObservedHostmask {
    /// The `nick!user@host` the identity was seen from.
    pub hostmask: String,
    /// When the identity was first seen from this hostmask.
    pub first_seen: DateTime<Utc>,
    /// When the identity was last seen from this hostmask.
    pub last_seen: DateTime<Utc>,
}
