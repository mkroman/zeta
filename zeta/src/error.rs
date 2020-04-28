use std::fmt;

#[derive(Debug)]
pub enum ConfigError {
    YamlError(serde_yaml::Error),
}

#[derive(Debug)]
pub enum Error {
    /// An error occurred when processing the config file
    ConfigError(ConfigError),
    /// An error occurred that was propagated from the core
    InternalError(zeta_core::Error),
    /// There was no configuration entry for the provided environment
    NoSuchEnvironmentError,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ConfigError::YamlError(ref err) => write!(f, "YAML parsing error: {}", err),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Error::ConfigError(ref err) => write!(f, "Config error: {}", err),
            Error::InternalError(ref err) => write!(f, "Core error: {}", err),
            Error::NoSuchEnvironmentError => {
                write!(f, "No matching environment found in config file")
            }
        }
    }
}

impl From<zeta_core::Error> for Error {
    fn from(err: zeta_core::Error) -> Error {
        Error::InternalError(err)
    }
}

impl From<ConfigError> for Error {
    fn from(err: ConfigError) -> Error {
        Error::ConfigError(err)
    }
}
