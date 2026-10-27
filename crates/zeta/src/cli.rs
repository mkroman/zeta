use std::path::PathBuf;

use clap::Parser;

/// Zeta IRC bot
#[derive(Parser, Debug)]
pub struct Opts {
    /// the path to the config file
    #[arg(short = 'c', long, default_value = "config.toml")]
    pub config_path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        // clap reports contradictory definitions through asserts; evaluating them here turns a
        // bad definition into a test failure instead of a panic in debug builds.
        Opts::command().debug_assert();
    }

    #[test]
    fn config_path_defaults_and_overrides() {
        let options = Opts::try_parse_from(["zeta"]).expect("the defaults parse");
        assert_eq!(options.config_path, PathBuf::from("config.toml"));

        let options =
            Opts::try_parse_from(["zeta", "-c", "other.toml"]).expect("`-c` overrides the path");
        assert_eq!(options.config_path, PathBuf::from("other.toml"));

        let options = Opts::try_parse_from(["zeta", "--config-path", "other.toml"])
            .expect("`--config-path` overrides the path");
        assert_eq!(options.config_path, PathBuf::from("other.toml"));
    }
}
