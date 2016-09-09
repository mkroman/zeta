#![feature(associated_consts)]

#[macro_use] extern crate log;
#[macro_use] extern crate zeta;
extern crate scraper;
extern crate hyper;

use std::error::Error;

use zeta::env_logger;
use zeta::plugin::PluginManager;

mod google_search;

pub mod util {
    pub fn split_command(message: &str) -> (&str, Option<&str>) {
        let v: Vec<&str> = message.splitn(2, ' ').collect();

        if v.len() > 1 && !v[1].is_empty() {
            (&v[0], Some(&v[1]))
        } else {
            (&v[0], None)
        }
    }
}

#[no_mangle]
pub extern fn register_plugins(plugins: &mut PluginManager) {
    match env_logger::init() {
        Ok(()) => {},
        Err(ref err) => {
            println!("Error when trying to initialize logger: {}", err.description());
        }
    }

    let mut count = 0;

    macro_rules! register_mod {
        ($name:ident) => {
            let plugin = $name::register(plugins);
            info!("Loaded plugin {:?} (v{})", plugin.name(), plugin.version());
            count += 1;
        }
    }

    register_mod!(google_search);
    debug!("Loaded {} plugins", count);
}

#[no_mangle]
pub extern fn unregister_plugins(_: &mut PluginManager) {
}
