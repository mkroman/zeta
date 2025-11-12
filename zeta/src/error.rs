//! Error types

use miette::Diagnostic;
use thiserror::Error;

pub use irc::error::Error as IrcError;
pub use sqlx::{Error as SqlxError, migrate::MigrateError as SqlxMigrateError};
pub use zeta_plugin::Error as PluginError;

/// Application errors for database, IRC, and plugin operations.
#[derive(Error, Debug, Diagnostic)]
pub enum Error {
    /// Failed to establish a connection to the database.
    #[error("Cannot connect to database")]
    OpenDatabase(#[source] SqlxError),
    /// Failed to create the IRC client.
    #[error("Could not create IRC client")]
    IrcClient(#[source] IrcError),
    /// Failed to register with the IRC server.
    #[error("Could not send registration details for IRC")]
    IrcRegistration(#[source] irc::error::Error),
    /// Failed to acquire a database connection from the connection pool.
    #[error("Could not acquire a connection from the connection pool")]
    DatabasePool(#[source] SqlxError),
    /// Database schema migration failed.
    #[error("Database migration failed")]
    DatabaseMigration(#[source] SqlxMigrateError),
    /// A database query operation failed.
    #[error("Database query failed")]
    DatabaseQueryFailed(#[from] SqlxError),
    /// General IRC communication error.
    #[error("IRC error")]
    Irc(#[from] IrcError),
    /// Plugin system error.
    #[error("Plugin error: {0}")]
    Plugin(#[from] PluginError),
}
