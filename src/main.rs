// Copyright (C) 2015 Mikkel Kroman <mk@maero.dk>
// All rights reserved.

#![feature(associated_consts)]
// #![feature(plugin)]
// #![plugin(clippy)]

extern crate semver;
extern crate irc;
// Crates used by the plugins.
extern crate hyper;

#[macro_use] mod plugin;
mod zeta;
mod server;

use std::thread::spawn;
use irc::client::prelude::*;

use zeta::Zeta;
use plugin::Plugin;

pub mod plugins {
    pub mod google_search;
}

fn main() {
    let mut zeta = Zeta::new();

    let guard = spawn(move || {
        zeta.connect().unwrap();
        zeta.initialize_plugins().run().unwrap();
    });

    guard.join().unwrap();
}
