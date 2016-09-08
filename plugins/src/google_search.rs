use zeta::plugin::prelude::*;

struct GoogleSearch;

impl Plugin for GoogleSearch {
    fn new() -> GoogleSearch {
        debug!("Google Search plugin initialized");

        GoogleSearch
    }

    fn process(&self, _: &IrcServer, _: &Message) -> Result<(), ()> {
        Ok(())
    }
}

impl Drop for GoogleSearch {
    fn drop(&mut self) {
        debug!("Google Search plugin uninitialized");
    }
}

plugin!(GoogleSearch, "Google Search", "0.1", "Allows a user to search Google", "Mikkel Kroman <mk@maero.dk>");

pub fn register(plugins: &mut PluginManager) -> &Box<Plugin> {
    plugins.register::<GoogleSearch>().unwrap()
}

