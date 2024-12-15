use std::time::Duration;

use sqlx::{
    migrate::Migrator,
    postgres::{PgPool, PgPoolOptions},
};

use crate::Error;

static MIGRATOR: Migrator = sqlx::migrate!();
pub const DEFAULT_MAX_CONNECTIONS: u32 = 5;
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub type Database = PgPool;

pub async fn connect(url: &str, config: &crate::config::DbConfig) -> Result<Database, Error> {
    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .idle_timeout(config.idle_timeout)
        .connect(url)
        .await
        .map_err(Error::OpenDatabase)?;

    Ok(pool)
}

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
