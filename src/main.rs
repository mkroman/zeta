use ::tracing::debug;
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use miette::IntoDiagnostic;

mod cli;
mod config;
mod database;
mod error;
mod tracing;
mod zeta;

use config::Config;
use error::Error;
use zeta::Zeta;

#[tokio::main]
async fn main() -> miette::Result<()> {
    let opts: cli::Opts = argh::from_env();
    let config: Config = Figment::new()
        .merge(Toml::file(opts.config_path))
        .merge(Env::prefixed("ZETA_").lowercase(false).split("__"))
        .extract()
        .into_diagnostic()?;

    tracing::init(&opts.format, &config.tracing)?;

    debug!("connecting to database");
    let db = database::connect(config.database.url.as_str(), &config.database).await?;
    debug!("connected to database");

    debug!("running database migrations");
    database::migrate(db.clone()).await?;
    debug!("database migrations complete");

    let z = Zeta::from_config(config);

    Ok(())
}
