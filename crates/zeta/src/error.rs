//! Error types

use miette::Diagnostic;
use thiserror::Error;

pub use irc::error::Error as IrcError;

#[cfg(feature = "database")]
pub use sqlx::{Error as SqlxError, migrate::MigrateError as SqlxMigrateError};

#[cfg(any(feature = "http", feature = "mirror"))]
pub use zeta_http::RequestError;
#[cfg(feature = "emulated")]
pub use zeta_http::WreqError;

/// Application errors for database, IRC, and plugin operations.
#[derive(Error, Debug, Diagnostic)]
pub enum Error {
    /// Failed to establish a connection to the database.
    #[error("Cannot connect to database")]
    #[cfg(feature = "database")]
    OpenDatabase(#[source] SqlxError),
    /// Failed to create the IRC client.
    #[error("Could not create IRC client")]
    IrcClient(#[source] IrcError),
    /// Failed to register with the IRC server.
    #[error("Could not send registration details for IRC")]
    IrcRegistration(#[source] irc::error::Error),
    /// Failed to acquire a database connection from the connection pool.
    #[error("Could not acquire a connection from the connection pool")]
    #[cfg(feature = "database")]
    DatabasePool(#[source] SqlxError),
    /// Database schema migration failed.
    #[error("Database migration failed")]
    #[cfg(feature = "database")]
    DatabaseMigration(#[source] SqlxMigrateError),
    /// General IRC communication error.
    #[error("IRC error")]
    Irc(#[from] IrcError),
}
