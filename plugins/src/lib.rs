#![feature(associated_consts)]

#[macro_use] extern crate log;
#[macro_use] extern crate zeta;

use zeta::env_logger;
use zeta::plugin::Plugin;
use zeta::plugin::PluginManager;

mod google_search;

#[no_mangle]
pub extern fn register_plugins(plugins: &mut PluginManager) {
    env_logger::init().unwrap();

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
pub extern fn unregister_plugins(plugins: &mut PluginManager) {
    plugins.clear();
}
