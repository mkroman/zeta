use std::fs;
use std::path::Path;

mod error;
use error::{ConfigError, Error};

use argh::FromArgs;
use miette::{IntoDiagnostic, WrapErr};
use tracing::{trace, warn};

use zeta_core::config::{Config, ConfigMap};
use zeta_core::Core;

/// Loads the given `path` as a YAML configuration file.
fn load_config(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let path = path.as_ref();

    trace!(?path, "Loading config file");

    let file = fs::File::open(path)?;
    let config: zeta_core::Config = serde_yaml::from_reader(&file)?;

    trace!(?path, "Successfully loaded config file");

    Ok(config)
}

/// Loads the given `path` as a configuration file while attempting to extract the `ConfigMap` for
/// the given environment `env`.
fn load_config_env(path: impl AsRef<Path>, env: &str) -> Result<ConfigMap, Error> {
    let path = path.as_ref();

    let mut config = load_config(path).map_err(|e| Error::LoadConfigError {
        path: path.display().to_string(),
        source: e,
    })?;

    let config_map = config.remove(env).ok_or(Error::NoSuchEnvironmentError)?;

    Ok(config_map)
}

/// Hello
#[derive(Debug, FromArgs)]
struct Opts {
    /// path to config file
    #[argh(option, default = "String::from(\"config.yaml\")")]
    config_path: String,
    /// the configuration environment to use
    #[argh(option, default = "String::from(\"development\")")]
    environment: String,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Parse command-line arguments
    let opts: Opts = argh::from_env();

    println!(
        "{} v{} running",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );

    // Load the config file and extract the environment-specific values from it
    let config_path = std::path::PathBuf::from(opts.config_path);
    let config_map = load_config_env(config_path, &opts.environment).into_diagnostic()?;

    // Create the core and add the networks
    let mut core = Core::new();

    trace!(
        "Adding {} network(s) to the core",
        config_map.networks.len()
    );

    for network in config_map.networks.iter() {
        trace!(%network.url, "Adding network");

        core.add_network(network.clone()).into_diagnostic()?;
    }

    trace!(
        num_networks = core.num_networks(),
        "Successfully added network(s)"
    );
    trace!("Booting up the core");

    let err = core
        .poll()
        .await
        .into_diagnostic()
        .with_context(|| "Polling main core");

    warn!("The core stopped polling");

    err.map(|_| ())
}
