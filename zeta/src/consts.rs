use std::time::Duration;

/// The `User-Agent` header to send when issuing HTTP requests.
pub const HTTP_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:155.0) Gecko/20100101 Firefox/155.0";

/// The duration before a HTTP request times out.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// The default value for the maximum number of connections the database connection pool will keep
/// open at once.
pub const DEFAULT_MAX_DB_CONNECTIONS: u32 = 5;

/// The default value for the duration the connection pool will keep an idle connection open.
pub const DEFAULT_DB_IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// The port number to use for plain, unencrypted IRC connections when not otherwise specified.
pub const DEFAULT_IRC_PORT: u16 = 6667;

/// The port number to use for secure IRC connections when not otherwise specified.
pub const DEFAULT_IRC_TLS_PORT: u16 = 6697;

/// The duration after a shutdown signal within which the plugin tasks are expected to finish
/// their queued messages and shutdown hooks, before they are aborted.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// The default `QUIT` message sent when the bot shuts down.
pub const DEFAULT_SHUTDOWN_QUIT_MESSAGE: &str = "shutting down";
