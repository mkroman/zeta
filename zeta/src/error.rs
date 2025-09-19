//! Error types

use miette::Diagnostic;
use thiserror::Error;

/// Application errors for database, IRC, and plugin operations.
#[derive(Error, Debug, Diagnostic)]
pub enum Error {
    /// Failed to establish a connection to the database.
    #[error("Cannot connect to database")]
    OpenDatabase(#[source] sqlx::Error),
    /// Failed to create the IRC client.
    #[error("Could not create IRC client")]
    IrcClient(#[source] irc::error::Error),
    /// Failed to register with the IRC server.
    #[error("Could not send registration details for IRC")]
    IrcRegistration(#[source] irc::error::Error),
    /// Failed to acquire a database connection from the connection pool.
    #[error("Could not acquire a connection from the connection pool")]
    DatabasePool(#[source] sqlx::Error),
    /// Database schema migration failed.
    #[error("Database migration failed")]
    DatabaseMigration(#[source] sqlx::migrate::MigrateError),
    /// A database query operation failed.
    #[error("Database query failed")]
    DatabaseQueryFailed(#[from] sqlx::Error),
    /// General IRC communication error.
    #[error("IRC error")]
    Irc(#[from] irc::error::Error),
    /// Plugin system error.
    #[error("Plugin error: {0}")]
    Plugin(Box<dyn std::error::Error + Send + Sync>),
}
