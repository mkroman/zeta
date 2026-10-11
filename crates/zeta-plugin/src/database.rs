//! Database helpers shared with the plugins.
//!
//! The host connects the pool through [`connect`] and applies the plugins' migrations at
//! startup; plugins reach the pool through their context's [`Database`] handle and persist
//! their state through repositories built on the [`DbError`] and [`database_error!`] helpers
//! here.

use std::time::Duration;

/// Database connection pool.
pub type Database = sqlx::PgPool;

/// Pool sizing options for [`connect`].
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// Maximum number of connections the pool will keep open at once.
    pub max_connections: u32,
    /// How long the pool keeps an idle connection open.
    pub idle_timeout: Duration,
    /// The maximum lifetime of individual connections before they are reaped.
    pub max_lifetime: Duration,
}

/// Connects to the database using `url` and the given pool sizing.
///
/// The connection is established lazily: the pool is returned before any connection has been
/// made, and the first query drives the actual connect.
///
/// # Errors
///
/// Returns the `sqlx::Error` if the pool cannot be created.
pub async fn connect(url: &str, config: &PoolConfig) -> Result<Database, sqlx::Error> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(config.max_connections)
        .idle_timeout(config.idle_timeout)
        .connect(url)
        .await
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
#[derive(Debug)]
pub struct DbError {
    stage: Stage,
    entity: &'static str,
    source: sqlx::Error,
}

/// The stage of a database operation that failed.
#[derive(Debug, Clone, Copy)]
enum Stage {
    /// Loading rows from the database.
    Load,
    /// Inserting a row into the database.
    Insert,
    /// Deleting rows from the database.
    Delete,
}

impl DbError {
    /// Constructs the error for a failed load of `entity` rows.
    #[must_use]
    pub const fn load(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new(Stage::Load, entity, source)
    }

    /// Constructs the error for a failed insert of an `entity` row.
    #[must_use]
    pub const fn insert(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new(Stage::Insert, entity, source)
    }

    /// Constructs the error for a failed delete of `entity` rows.
    #[must_use]
    pub const fn delete(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new(Stage::Delete, entity, source)
    }

    /// Constructs the error from its parts.
    const fn new(stage: Stage, entity: &'static str, source: sqlx::Error) -> Self {
        Self {
            stage,
            entity,
            source,
        }
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.stage {
            Stage::Load => write!(f, "could not load {} from database", self.entity),
            Stage::Insert => write!(f, "could not insert {}: {}", self.entity, self.source),
            Stage::Delete => write!(f, "could not delete {}: {}", self.entity, self.source),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Generates the database error constructors for a repository error enum.
///
/// The generated `load`, `insert` and `delete` constructors wrap a [`sqlx::Error`] into a
/// [`DbError`] naming `$entity`, wrapped into `$error::Database` — the variant every repository
/// error enum carries. The macro keeps that wiring in one place instead of three near-identical
/// constructor blocks per plugin.
#[macro_export]
macro_rules! database_error {
    ($error:ty, $entity:literal) => {
        impl $error {
            /// Constructs the error for a failed load from the database.
            pub(crate) const fn load(source: sqlx::Error) -> Self {
                Self::Database($crate::database::DbError::load($entity, source))
            }

            /// Constructs the error for a failed insert into the database.
            pub(crate) const fn insert(source: sqlx::Error) -> Self {
                Self::Database($crate::database::DbError::insert($entity, source))
            }

            /// Constructs the error for a failed delete from the database.
            pub(crate) const fn delete(source: sqlx::Error) -> Self {
                Self::Database($crate::database::DbError::delete($entity, source))
            }
        }
    };
}

pub use crate::database_error;
