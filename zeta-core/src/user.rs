use std::sync::Arc;

pub use crate::Channel;

/// This struct contains details about a user, such as its nickname, username, hostname, what
/// channel it is known to be in, etc.
#[derive(Debug, Clone, Default)]
pub struct User {
    // The users nickname
    nick: String,
    // The users username
    name: String,
    // The users hostname
    host: String,
    // A list of atomically reference-counted channels that the user is in
    channels: Vec<Arc<Channel>>,
}

impl User {
    pub fn new<S: Into<String>>(nick: S) -> User {
        User {
            nick: nick.into(),
            ..Default::default()
        }
    }

    /// Returns the users nickname
    pub fn nick(&self) -> &str {
        &self.nick
    }

    /// Returns the users username
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the users hostname
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns a list of channels that the user is currently known to be in
    pub fn channels(&self) -> &Vec<Arc<Channel>> {
        &self.channels
    }
}
