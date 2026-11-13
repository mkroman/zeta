//! Database helpers shared by the DB-backed plugins.
//!
//! The host connects the pool and applies the migrations; the plugins' repositories reach it
//! through their context and persist their state through the [`DbError`] and
//! [`impl_repository_error!`](crate::impl_repository_error) helpers here.

use std::future::Future;
use std::time::Duration;

use tracing::warn;

use crate::event::Sender;

/// Database connection pool.
pub type Database = sqlx::PgPool;

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
/// [`LOAD_ATTEMPTS`] attempts in total.
///
/// # Errors
///
/// Returns the error of the last failed `load` attempt.
pub async fn load_with_retry<E, F, Fut>(mut load: F) -> Result<(), E>
where
    E: std::fmt::Display,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), E>>,
{
    let mut delay = LOAD_BACKOFF;

    for attempt in 1..=LOAD_ATTEMPTS {
        match load().await {
            Ok(()) => return Ok(()),
            Err(error) if attempt < LOAD_ATTEMPTS => {
                warn!(
                    attempt,
                    max_attempts = LOAD_ATTEMPTS,
                    exception.message = %error,
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

/// The attribution of an inserted row to the user and channel it came from.
///
/// The `(nickname, username, hostname, channel)` columns are repeated by every plugin that
/// records who posted a link or created a row, so the `New*` insert models carry them as one
/// field. Row models keep the four columns as their own fields: they derive `FromRow` from the
/// queried column order, which the attribution does not change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribution {
    /// The nickname of the user.
    pub nickname: String,
    /// The username (ident) of the user.
    pub username: String,
    /// The hostname of the user.
    pub hostname: String,
    /// The channel the user acted in.
    pub channel: String,
}

impl Attribution {
    /// Constructs an attribution from its parts.
    #[must_use]
    pub fn new(nickname: &str, username: &str, hostname: &str, channel: &str) -> Self {
        Self {
            nickname: nickname.to_owned(),
            username: username.to_owned(),
            hostname: hostname.to_owned(),
            channel: channel.to_owned(),
        }
    }

    /// Attributes `channel` to `sender`.
    #[must_use]
    pub fn from_sender(sender: Sender<'_>, channel: &str) -> Self {
        Self::new(sender.nick, sender.username, sender.hostname, channel)
    }
}

/// Deletes the rows with the given `ids` from `table`, returning the number of deleted rows.
///
/// `table` must be a table known at compile time; it is never sourced from user input.
///
/// # Errors
///
/// Returns the `sqlx::Error` if the query fails.
pub async fn delete_ids(
    db: &Database,
    table: &'static str,
    ids: &[i32],
) -> Result<u64, sqlx::Error> {
    // The table name is a compile-time constant from the calling module, never user input.
    let query = sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE id = ANY($1)"));

    sqlx::query(query)
        .bind(ids)
        .execute(db)
        .await
        .map(|result| result.rows_affected())
}

/// A failed database operation, naming the affected entity.
#[derive(Debug, thiserror::Error)]
#[error("could not {stage} {entity}: {source}")]
pub struct DbError {
    stage: &'static str,
    entity: &'static str,
    source: sqlx::Error,
}

impl DbError {
    /// Constructs the error for a failed load of `entity` rows.
    #[must_use]
    pub const fn load(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new("load", entity, source)
    }

    /// Constructs the error for a failed insert of an `entity` row.
    #[must_use]
    pub const fn insert(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new("insert", entity, source)
    }

    /// Constructs the error for a failed delete of `entity` rows.
    #[must_use]
    pub const fn delete(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new("delete", entity, source)
    }

    /// Constructs the error from its parts.
    const fn new(stage: &'static str, entity: &'static str, source: sqlx::Error) -> Self {
        Self {
            stage,
            entity,
            source,
        }
    }
}

/// Generates a repository struct backed by the shared database pool.
///
/// ```ignore
/// zeta_plugin::impl_repository! {
///     /// Repository for storing and retrieving alerts in the database.
///     AlertRepository
/// }
/// ```
///
/// The generated struct has a private `db` field and a `pub const fn new(db: Database)`, so the
/// repository's own `impl` block contains only queries.
#[macro_export]
macro_rules! impl_repository {
    (
        $(#[$meta:meta])*
        $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $name {
            /// The database connection pool.
            db: $crate::database::Database,
        }

        impl $name {
            /// Creates a new repository backed by the given database pool.
            #[must_use]
            pub const fn new(db: $crate::database::Database) -> Self {
                Self { db }
            }
        }
    };
}

/// Generates the database error constructors for a repository error enum.
///
/// The generated `load`, `insert` and `delete` constructors wrap a [`sqlx::Error`] into a
/// [`DbError`] naming `$entity`, wrapped into `$error::Database` — the variant every repository
/// error enum carries. The macro keeps that wiring in one place instead of three near-identical
/// constructor blocks per plugin.
#[macro_export]
macro_rules! impl_repository_error {
    ($error:ty, $entity:literal) => {
        impl $error {
            /// Constructs the error for a failed load from the database.
            pub(crate) const fn load(source: $crate::sqlx::Error) -> Self {
                Self::Database($crate::database::DbError::load($entity, source))
            }

            /// Constructs the error for a failed insert into the database.
            pub(crate) const fn insert(source: $crate::sqlx::Error) -> Self {
                Self::Database($crate::database::DbError::insert($entity, source))
            }

            /// Constructs the error for a failed delete from the database.
            pub(crate) const fn delete(source: $crate::sqlx::Error) -> Self {
                Self::Database($crate::database::DbError::delete($entity, source))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A transient load failure is retried with a doubled delay: 250 ms, then 500 ms, before the
    /// third attempt succeeds.
    #[tokio::test(start_paused = true)]
    async fn retries_a_transient_load_failure() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&attempts);
        let started = tokio::time::Instant::now();

        let result = load_with_retry(|| {
            let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;

            async move {
                if attempt < 3 {
                    Err(format!("attempt {attempt}"))
                } else {
                    Ok(())
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert!(
            started.elapsed() >= Duration::from_millis(750),
            "250 ms + 500 ms of backoff, elapsed {:?}",
            started.elapsed()
        );
    }

    /// A persistent failure is returned only after every attempt, so the error logged by the
    /// plugin task is the one from the last attempt.
    #[tokio::test(start_paused = true)]
    async fn a_persistent_load_failure_is_returned_after_every_attempt() {
        let attempts = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&attempts);
        let started = tokio::time::Instant::now();

        let result = load_with_retry(|| {
            let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;

            async move { Err::<(), _>(format!("attempt {attempt}")) }
        })
        .await;

        assert_eq!(attempts.load(Ordering::SeqCst), LOAD_ATTEMPTS);
        assert_eq!(result.unwrap_err(), format!("attempt {LOAD_ATTEMPTS}"));
        assert!(
            started.elapsed() >= Duration::from_millis(1_750),
            "250 ms + 500 ms + 1 s of backoff, elapsed {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn db_error_display_and_source_match_the_manual_impl() {
        let cases = [
            ("load", DbError::load("alert", sqlx::Error::RowNotFound)),
            ("insert", DbError::insert("alert", sqlx::Error::RowNotFound)),
            ("delete", DbError::delete("alert", sqlx::Error::RowNotFound)),
        ];

        for (stage, error) in cases {
            // The manual `Display` wrote `could not {stage} {entity}: {source}`.
            assert_eq!(
                error.to_string(),
                format!("could not {stage} alert: {}", error.source)
            );

            // A field named `source` is the error's source, as the manual impl returned it.
            let source = std::error::Error::source(&error).expect("the sqlx error is the source");

            assert!(std::ptr::addr_eq(
                &raw const *source,
                &raw const error.source,
            ));
        }
    }
}
