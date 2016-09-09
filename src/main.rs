extern crate zeta;
extern crate clap;
extern crate env_logger;

use std::error::Error;

use clap::{Arg, App};
use zeta::Zeta;

fn parse_options<'a>() -> clap::ArgMatches<'a> {
    App::with_defaults("zeta")
        .author(env!("CARGO_PKG_AUTHORS"))
        .version(env!("CARGO_PKG_VERSION"))
        .arg(Arg::with_name("config")
             .long("config")
             .short("c")
             .takes_value(true)
             .help("set configuration file (default: config.json)")
             .value_name("FILE"))
        .get_matches()
}

fn main() {
    env_logger::init().unwrap();

    let matches = parse_options();
    let config_path = matches.value_of("config").unwrap_or("config.json");

    match Zeta::new(&config_path) {
        Ok(mut zeta) => {
            zeta.connect().unwrap();
            zeta.load_plugins().expect("could not load plugins");
            zeta.run().unwrap();
        },
        Err(e) => {
            println!("A critical error occurred!");
            println!("{}: {}", &e, e.description());
        }
    }
}
