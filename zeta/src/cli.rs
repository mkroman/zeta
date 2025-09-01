use std::{fmt, path::PathBuf, str::FromStr};

use argh::FromArgs;

/// The output format of logging messages to stdout.
#[derive(Debug, Clone, Default)]
pub enum Format {
    /// Output events as JSON.
    Json,
    /// Output events in an excessively human-readable format.
    Pretty,
    /// Output events in a human-readable format.
    #[default]
    Compact,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json => write!(f, "json"),
            Self::Pretty => write!(f, "pretty"),
            Self::Compact => write!(f, "compact"),
        }
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(Self::Json),
            "pretty" => Ok(Self::Pretty),
            "compact" => Ok(Self::Compact),
            _ => Err("unsupported format, must be one of json, pretty, compact, full".to_string()),
        }
    }
}

/// Masked mails server
#[derive(FromArgs, Debug)]
pub struct Opts {
    /// the path to the config file
    #[argh(option, short = 'c', default = r#"PathBuf::from("config.toml")"#)]
    pub config_path: PathBuf,
    /// logging output format to stdout
    #[argh(option, default = "Format::default()")]
    pub format: Format,
}
