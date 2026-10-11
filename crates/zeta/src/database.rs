//! Database integration.
//!
//! Connects the pool (through [`zeta_plugin::database`]) and applies the plugins' migrations at
//! startup; the pool itself is shared with plugins through their context, and the repository
//! helpers live in [`zeta_plugin::database`].

use sqlx::migrate::Migrator;

use crate::Error;

pub use zeta_plugin::database::Database;

/// The plugins' database migrations, applied at startup.
static MIGRATOR: Migrator = sqlx::migrate!("../plugins/migrations");

/// Connects to the database using the provided url and configuration.
///
/// # Errors
///
/// If unable to establish connection to the database, `Err(Error::OpenDatabase)` is returned.
pub async fn connect(url: &str, config: &crate::config::DbConfig) -> Result<Database, Error> {
    let pool = zeta_plugin::database::connect(
        url,
        &zeta_plugin::database::PoolConfig {
            max_connections: config.max_connections,
            idle_timeout: config.idle_timeout,
            max_lifetime: config.max_lifetime,
        },
    )
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
