use std::sync::Arc;

use crate::User;

/// This structure contains details about a channel
#[derive(Debug, Clone, Default)]
pub struct Channel {
    // The channel name
    name: String,
    // The channels current topic
    topic: String,
    // A list of atomically reference-counted users present in this channel
    users: Vec<Arc<User>>,
}

impl Channel {
    pub fn new<S: Into<String>>(name: S) -> Channel {
        Channel {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Returns the name of the channel
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the topic of the channel
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Returns a list of users currently known to be present in this channel
    pub fn users(&self) -> &Vec<Arc<User>> {
        &self.users
    }
}
