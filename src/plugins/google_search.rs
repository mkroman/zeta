// Copyright (C) 2015 Mikkel Kroman <mk@maero.dk>
// All rights reserved.

use hyper::Client;
use plugin::prelude::*;

// Google search plugin state.
pub struct Context {
    http_client: ::hyper::Client,
}

impl Plugin for Context {
    fn new() -> Context {
        Context {
            http_client: Client::new()
        }
    }

    fn process<'a>(&self, server: &'a NetIrcServer, cmd: &Command) -> Result<(), ()> {
        match cmd {
            &Command::PRIVMSG(ref target, ref message) => {
                server.send_privmsg(target, format!("I am {:?}", &self).as_ref());
            },
            _ => {}
        }

        Ok(())
    }
}

impl Context {
}

plugin!(Context,
        "Google Search",
        "0.1.0", 
        "Send queries to Google",
        "Mikkel Kroman <mk@maero.dk>");

