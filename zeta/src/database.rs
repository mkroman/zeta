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
    let mut conn = pool
        .acquire()
        .await
        .map_err(Error::AcquireDatabaseConnection)?;

    MIGRATOR
        .run(&mut conn)
        .await
        .map_err(Error::DatabaseMigration)
}
