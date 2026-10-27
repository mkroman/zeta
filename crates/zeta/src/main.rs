//! Zeta is an opinionated IRC bot with a bunch of plugins.

mod cli;
mod telemetry;

use clap::Parser;
#[cfg(feature = "database")]
use zeta::database;
use zeta::{Config, Zeta};

#[tokio::main]
async fn main() -> miette::Result<()> {
    // Install the process-level rustls CryptoProvider explicitly, before anything (sqlx, irc,
    // reqwest, hickory) touches TLS, instead of relying on rustls picking a default provider from
    // the features enabled across the dependency graph.
    #[cfg(feature = "rustls")]
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install default rustls CryptoProvider");

    let opts = cli::Opts::parse();
    let config = Config::load(opts.config_path)?;

    telemetry::try_init(&config.tracing)?;

    #[cfg(feature = "database")]
    let db = {
        use tracing::debug;

        debug!("connecting to database");
        let db = database::connect(&config.database).await?;
        debug!("connected to database");

        debug!("running database migrations");
        database::migrate(&db).await?;
        debug!("database migrations complete");
        db
    };

    Zeta::new(
        config,
        #[cfg(feature = "database")]
        db,
    )
    .run()
    .await?;

    Ok(())
}
