// Copyright (c) 2015, Mikkel Kroman <mk@uplink.io>
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are met:
//
// * Redistributions of source code must retain the above copyright notice, this
//   list of conditions and the following disclaimer.
//
// * Redistributions in binary form must reproduce the above copyright notice,
//   this list of conditions and the following disclaimer in the documentation
//   and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
// AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
// FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
// CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
// OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

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
