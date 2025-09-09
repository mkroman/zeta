use std::path::PathBuf;

use argh::FromArgs;

/// Masked mails server
#[derive(FromArgs, Debug)]
pub struct Opts {
    /// the path to the config file
    #[argh(option, short = 'c', default = r#"PathBuf::from("config.toml")"#)]
    pub config_path: PathBuf,
}
