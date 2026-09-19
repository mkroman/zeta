use sqlx::{
    migrate::Migrator,
    postgres::{PgPool, PgPoolOptions},
};

use crate::Error;

static MIGRATOR: Migrator = sqlx::migrate!();

/// Database connection pool.,
pub type Database = PgPool;

/// Connects to the database using the provided url and configuration.
///
/// # Errors
///
/// If unable to establish connection to the database, `Err(Error::OpenDatabase)` is returned.
pub async fn connect(url: &str, config: &crate::config::DbConfig) -> Result<Database, Error> {
    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .idle_timeout(config.idle_timeout)
        .connect(url)
        .await
        .map_err(Error::OpenDatabase)?;

    Ok(pool)
}

/// Applies migrations to the database.
///
/// # Errors
///
/// If a connection cannot be acquired from the connection pool, `Error::AcquireDatabaseConnection`
/// is returned.
///
/// If an error occurs during migration, `Error::DatabaseMigration` is returned.
pub async fn migrate(pool: Database) -> Result<(), Error> {
    let mut conn = pool.acquire().await.map_err(Error::DatabasePool)?;

    MIGRATOR
        .run(&mut conn)
        .await
        .map_err(Error::DatabaseMigration)
}

/// Deletes the rows with the given `ids` from `table`, returning the number of deleted rows.
///
/// `table` must be a table known at compile time; it is never sourced from user input.
///
/// # Errors
///
/// Returns the `sqlx::Error` if the query fails.
pub(crate) async fn delete_ids(
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
    pub(crate) const fn load(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new(Stage::Load, entity, source)
    }

    /// Constructs the error for a failed insert of an `entity` row.
    pub(crate) const fn insert(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new(Stage::Insert, entity, source)
    }

    /// Constructs the error for a failed delete of `entity` rows.
    pub(crate) const fn delete(entity: &'static str, source: sqlx::Error) -> Self {
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
macro_rules! database_error {
    ($error:ty, $entity:literal) => {
        impl $error {
            /// Constructs the error for a failed load from the database.
            pub(crate) const fn load(source: sqlx::Error) -> Self {
                Self::Database(DbError::load($entity, source))
            }

            /// Constructs the error for a failed insert into the database.
            pub(crate) const fn insert(source: sqlx::Error) -> Self {
                Self::Database(DbError::insert($entity, source))
            }

            /// Constructs the error for a failed delete from the database.
            pub(crate) const fn delete(source: sqlx::Error) -> Self {
                Self::Database(DbError::delete($entity, source))
            }
        }
    };
}

pub(crate) use database_error;

/// Connects to the test database, if one is configured.
///
/// Returns `None` when `ZETA_TEST_DATABASE_URL` is unset, so database-backed tests skip instead
/// of failing on machines without the database running.
#[cfg(test)]
pub(crate) async fn connect_for_tests() -> Option<Database> {
    use sqlx::postgres::PgPoolOptions;

    let url = std::env::var("ZETA_TEST_DATABASE_URL").ok()?;
    let db = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("could not connect to the test database");

    Some(db)
}
