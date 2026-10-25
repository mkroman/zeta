//! Database integration.
//!
//! The host owns the database story end to end: it connects the pool, applies the plugins'
//! migrations at startup, and hands the pool to the plugins through their context.

use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;

use crate::Error;

/// Database connection pool.
pub type Database = sqlx::PgPool;

/// The plugins' database migrations, applied at startup.
static MIGRATOR: Migrator = sqlx::migrate!("../plugins/migrations");

/// Connects to the database using `config`'s url and pool bounds.
///
/// # Errors
///
/// If unable to establish connection to the database, `Err(Error::OpenDatabase)` is returned.
pub async fn connect(config: &crate::config::DbConfig) -> Result<Database, Error> {
    let pool = PgPoolOptions::new()
        .min_connections(config.min_connections)
        .max_connections(config.max_connections)
        .idle_timeout(config.idle_timeout)
        .max_lifetime(config.max_lifetime)
        .connect(&config.url)
        .await
        .map_err(Error::OpenDatabase)?;

    Ok(pool)
}

/// Applies migrations to the database.
///
/// # Errors
///
/// If a connection cannot be acquired from the connection pool, `Error::DatabasePool` is returned.
///
/// If an error occurs during migration, `Error::DatabaseMigration` is returned.
pub async fn migrate(pool: &Database) -> Result<(), Error> {
    let mut conn = pool.acquire().await.map_err(Error::DatabasePool)?;

    MIGRATOR
        .run(&mut conn)
        .await
        .map_err(Error::DatabaseMigration)
}
