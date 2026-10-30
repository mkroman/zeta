//! Zeta is an opinionated IRC bot with a bunch of plugins.

mod cli;
mod telemetry;

use clap::Parser;
use tracing::Instrument;
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
    let config_path = opts.config_path;
    let config = Config::load(&config_path)?;

    // Held until `main` returns so the shutdown flushes the tracer provider's batches.
    let _telemetry = telemetry::try_init(&config.tracing)?;

    // The startup span covers configuration, database setup and plugin construction only. The
    // process lifetime is not a span: `run` intentionally has none, so each IRC line starts its
    // own trace instead of hanging under a weeks-long root.
    let startup_span = tracing::info_span!(
        "zeta.startup",
        config.path = %config_path.display(),
        irc.hostname = %config.irc.hostname,
        irc.nickname = %config.irc.nickname,
    );

    let zeta = async {
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

        Ok::<_, zeta::Error>(Zeta::new(
            config,
            #[cfg(feature = "database")]
            db,
        ))
    }
    .instrument(startup_span)
    .await?;

    zeta.run().await?;

    Ok(())
}
