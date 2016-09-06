use zeta::plugin::prelude::*;

struct GoogleSearch;

impl Plugin for GoogleSearch {
    fn new() -> GoogleSearch {
        println!("Google Search plugin initialized");

        GoogleSearch
    }

    fn process(&self, _: &IrcServer, _: &Message) -> Result<(), ()> {
        Ok(())
    }
}

plugin!(GoogleSearch, "Google Search", "0.1", "Allows a user to search Google", "Mikkel Kroman <mk@maero.dk>");

pub fn register(plugins: &mut PluginManager) -> &Box<Plugin> {
    plugins.register::<GoogleSearch>().unwrap()
}

