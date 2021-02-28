use std::fs;
use std::path::Path;

mod error;
use error::Error;

use clap::{crate_authors, crate_version, App, Arg};
use log::{debug, trace, warn};
use zeta_core::Core;

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

    let config_path = Path::new(matches.value_of("config").unwrap());

    trace!("Loading config file `{}'", config_path.display());

    let file = fs::File::open(config_path).map_err(|e| Error::LoadConfigError {
        path: config_path.display().to_string(),
        source: e.into(),
    })?;

    let parsed_config: zeta_core::Config =
        serde_yaml::from_reader(&file).map_err(|e| Error::LoadConfigError {
            path: config_path.display().to_string(),
            source: e.into(),
        })?;

    trace!("Successfully loaded config file");

    let config_map = parsed_config
        .get(matches.value_of("environment").unwrap())
        .ok_or(Error::NoSuchEnvironmentError)?;

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

    core.poll().await?;

    warn!("The core stopped polling");

    Ok(())
}
