use std::fs;
use std::path::Path;

mod error;
use error::{ConfigError, Error};

use clap::{crate_authors, crate_version, App, Arg};
use zeta_core::Core;

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Initialize logging
    env_logger::init();

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

    let config_path = Path::new(matches.value_of("config").unwrap());

    if !config_path.exists() {
        panic!("Config path does not exist");
    }

    let file = fs::File::open(config_path).expect("Could not open config file");
    let parsed_config: zeta_core::Config =
        serde_yaml::from_reader(&file).map_err(ConfigError::YamlError)?;

    let config_map = parsed_config
        .get(matches.value_of("environment").unwrap())
        .ok_or(Error::NoSuchEnvironmentError)?;

    let network_cfg = config_map.networks.first().expect("No networks defined");

    println!(
        "{} v{} running",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );

    let mut core = Core::new();

    core.connect(network_cfg.clone()).await?;
    core.poll().await?;

    Ok(())
}
