use std::time::Duration;

/// The `User-Agent` header to send when issuing HTTP requests.
pub const HTTP_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:141.0) Gecko/20100101 Firefox/141.0";

/// The duration before a HTTP request times out.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// The default value for the maximum number of connections the database connection pool will keep
/// open at once.
pub const DEFAULT_MAX_DB_CONNECTIONS: u32 = 5;

/// The default value for the duration the connection pool will keep an idle connection open.
pub const DEFAULT_DB_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
