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

extern crate irc;
extern crate url;
extern crate hyper;

use url::Url;
use hyper::Client;
use super::prelude::*;

// Google search plugin state.
pub struct Context {
    http_client: ::hyper::Client,
}

/// Google search result.
pub struct SearchResult {
    pub title: String,
    pub description: String,
    pub url: String,
}

impl Plugin for Context {
    fn new() -> Context {
        Context { http_client: Client::new() }
    }

    fn process(&self, server: &IrcServer, msg: &Message) -> Result<(), ()> {
        Ok(())
    }
}

impl Context {
    fn search(&self, query: &str) -> Result<SearchResult, &'static str> {
        let mut url = Url::parse("http://ajax.googleapis.com/ajax/services/search/web").unwrap();
        url.set_query_from_pairs(&[("q", query), ("v", "1.0"), ("rsz", "1")]);

        let res = self.http_client.get(url);
        println!("{:?}", res.send().unwrap());
        Ok(SearchResult {
            title: format!("d"),
            description: format!("d"),
            url: format!("d"),
        })
    }
}