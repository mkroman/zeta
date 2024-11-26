use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("yaml parsing error")]
    YamlError(#[from] serde_yaml::Error),
    #[error("i/o error")]
    IoError(#[from] io::Error),
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("could not load config file `{path}'")]
    LoadConfigError { path: String, source: ConfigError },
    /// An error occurred that was propagated from the core
    #[error("Zeta error")]
    InternalError(#[from] zeta_core::Error),
    /// There was no configuration entry for the provided environment
    #[error("No configuration map for given environment")]
    NoSuchEnvironmentError,
}
