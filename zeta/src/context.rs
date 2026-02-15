use hickory_resolver::TokioResolver;

use crate::Config;
#[cfg(feature = "database")]
use crate::database::Database;

/// Shared context for plugin invocations.
pub struct Context {
    /// The database connection pool.
    #[cfg(feature = "database")]
    pub db: Database,
    /// The DNS resolver.
    pub dns: TokioResolver,
    /// The bot configuration.
    pub config: Config,
}

impl Context {
    /// Creates a new context.
    #[must_use]
    pub const fn new(
        #[cfg(feature = "database")] db: Database,
        dns: TokioResolver,
        config: Config,
    ) -> Self {
        Self {
            #[cfg(feature = "database")]
            db,
            dns,
            config,
        }
    }
}
