//! Error types

use miette::Diagnostic;
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum Error {
    #[error("Cannot connect to database database")]
    #[diagnostic(code(zeta::db_open))]
    OpenDatabase(#[source] sqlx::Error),
    #[error("Could not acquire a connection from the connection pool")]
    AcquireDatabaseConnection(#[source] sqlx::Error),
    #[error("Database migration failed")]
    DatabaseMigration(#[source] sqlx::migrate::MigrateError),
    #[error("Database query failed")]
    DatabaseQueryFailed(#[from] sqlx::Error),
}
