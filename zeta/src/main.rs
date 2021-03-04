use std::fs;
use std::path::Path;

mod error;
use error::{ConfigError, Error};

use anyhow::Context;
use clap::{crate_authors, crate_version, App, Arg};
use log::{trace, warn};
use zeta_core::config::{Config, ConfigMap};
use zeta_core::Core;

/// Loads the given `path` as a YAML configuration file.
fn load_config(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let path = path.as_ref();

    trace!("Loading config file `{}'", path.display());

    let file = fs::File::open(path)?;
    let config: zeta_core::Config = serde_yaml::from_reader(&file)?;

    trace!("Successfully loaded config file");

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

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // Initialize logging
    pretty_env_logger::init_timed();

    // Parse command-line arguments
    let matches = App::new("Zeta")
        .version(crate_version!())
        .author(crate_authors!())
        .about("World's best IRC bot")
        .arg(
            Arg::with_name("config")
                .default_value("config.yml")
                .env("ZETA_CONFIG_PATH")
                .help("Path to config file")
                .long("config")
                .required(true)
                .short("c")
                .value_name("FILE"),
        )
        .arg(
            Arg::with_name("environment")
                .default_value("development")
                .env("ZETA_ENV")
                .help("The configuration environment to run")
                .long("env")
                .short("e"),
        )
        .get_matches();

    println!(
        "{} v{} running",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );

    // Load the config file and extract the environment-specific values from it
    let config_path = Path::new(matches.value_of("config").unwrap());
    let config_map = load_config_env(config_path, matches.value_of("environment").unwrap())?;

    // Create the core and add the networks
    let mut core = Core::new();

    trace!(
        "Adding {} network(s) to the core",
        config_map.networks.len()
    );

    for network in config_map.networks.iter() {
        trace!("Adding network {}", network.url);

        core.add_network(network.clone())?;
    }

    trace!("Successfully added {} network(s)", core.num_networks());
    trace!("Booting up the core");

    let err = core.poll().await.with_context(|| "Polling main core");

    warn!("The core stopped polling");

    err.map(|_| ())
}
