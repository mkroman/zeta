//! Error types

use miette::Diagnostic;
use thiserror::Error;

/// General application error.
#[derive(Error, Debug, Diagnostic)]
pub enum Error {
    #[error("Cannot connect to database database")]
    #[diagnostic(code(zeta::db_open))]
    OpenDatabase(#[source] sqlx::Error),
    #[error("Could not create IRC client")]
    IrcClientError(#[source] irc::error::Error),
    #[error("Could not send registration details for IRC")]
    IrcRegistrationError(#[source] irc::error::Error),
    #[error("Could not acquire a connection from the connection pool")]
    AcquireDatabaseConnection(#[source] sqlx::Error),
    #[error("Database migration failed")]
    DatabaseMigration(#[source] sqlx::migrate::MigrateError),
    #[error("Database query failed")]
    DatabaseQueryFailed(#[from] sqlx::Error),
    #[error("IRC error")]
    IrcError(#[from] irc::error::Error),
    #[error("Plugin error: {0}")]
    PluginError(Box<dyn std::error::Error + Send + Sync>),
    #[error("invalid configuration for plugin {0}: {1}")]
    PluginConfig(&'static str, #[source] Box<figment::Error>),
}
