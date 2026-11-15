//! Startup load retry and pool namespace helpers.
//!
//! [`load_with_retry`] waits out a transient database failure during a plugin's `loaded` hook,
//! and [`namespace`] reads the pool's database name for `db.namespace` span fields.

use std::future::Future;
use std::time::Duration;

use tracing::warn;

use crate::Chain;

use super::Database;

/// The number of times a failed startup load is attempted before the plugin gives up.
pub const LOAD_ATTEMPTS: u32 = 4;

/// The first delay between startup load attempts; each further delay doubles it.
pub const LOAD_BACKOFF: Duration = Duration::from_millis(250);

/// Loads a plugin's stored state with `load`, retrying a failed attempt with exponential
/// backoff.
///
/// A transient database failure during the `loaded` hook must not strand the plugin's stored
/// state for the rest of the process lifetime. The host waits for the hook before dispatching
/// (bounded by its startup grace), so retrying here keeps that window short instead of leaving
/// the plugin with empty in-memory state — e.g. no filters or no coverage — until the next
/// restart. Every failed attempt but the last is logged at `warn`; the error of the last attempt
/// is returned, so a `loaded` hook that still cannot load fails initialization as it would
/// without a retry (and the plugin task logs that error).
///
/// The delay starts at [`LOAD_BACKOFF`] and doubles after each failed attempt, for
/// [`LOAD_ATTEMPTS`] attempts in total. Each failed attempt logs `exception.message` through
/// [`Chain`], so the source detail of an error whose [`Display`](std::fmt::Display) is
/// reply-grade — like [`DbError`](super::DbError) — still reaches the operator log.
///
/// # Errors
///
/// Returns the error of the last failed `load` attempt.
pub async fn load_with_retry<E, F, Fut>(mut load: F) -> Result<(), E>
where
    E: std::error::Error + 'static,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), E>>,
{
    let mut delay = LOAD_BACKOFF;

    for attempt in 1..=LOAD_ATTEMPTS {
        match load().await {
            Ok(()) => return Ok(()),
            Err(error) if attempt < LOAD_ATTEMPTS => {
                warn!(
                    zeta.plugin.load.attempt = attempt,
                    zeta.plugin.load.max_attempts = LOAD_ATTEMPTS,
                    exception.message = %Chain(&error),
                    "startup load failed; retrying"
                );

                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("the last attempt returns")
}

/// Returns the name of the database the pool connects to, for `db.namespace` span fields.
#[must_use]
pub fn namespace(db: &Database) -> String {
    db.connect_options()
        .get_database()
        .unwrap_or_default()
        .to_owned()
}
