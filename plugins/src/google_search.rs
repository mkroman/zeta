use zeta::plugin::prelude::*;

use ::scraper::{Selector, Html};
use ::hyper::Client;

struct GoogleSearch {
    http_client: Client,
}

impl Plugin for GoogleSearch {
    fn new() -> GoogleSearch {
        GoogleSearch {
            http_client: Client::new(),
        }
    }

    fn process(&self, server: &IrcServer, message: &Message) {
        match message.command {
            Command::PRIVMSG(ref target, ref msg) => {
                match ::util::split_command(&msg) {
                    (".g", Some(ref args)) => {
                        server.send_privmsg(target, &format!("Sending query {:?}", args));
                    },
                    (".g", None) => {
                        server.send_privmsg(target, "> Usage: .g <query>");
                    },
                    _ => {}
                }
            },
            _ => {}
        }
    }
}

plugin!(GoogleSearch, "Google Search", "0.1", "Allows a user to search Google", "Mikkel Kroman <mk@maero.dk>");

pub fn register(plugins: &mut PluginManager) -> &Box<Plugin> {
    plugins.register::<GoogleSearch>().unwrap()
}

