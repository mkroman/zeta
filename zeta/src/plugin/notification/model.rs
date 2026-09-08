//! The data model of the notification plugin.

use sqlx::{
    prelude::FromRow,
    types::chrono::{DateTime, Utc},
};

/// A pending notification, as stored in the database.
// Some fields are only populated from the database and never read.
#[derive(Debug, FromRow, Clone)]
#[allow(dead_code)]
pub struct Notification {
    /// The database id of the notification.
    pub id: i32,
    /// The nickname the notification is aimed at.
    pub target: String,
    /// The nickname of the user who created the notification.
    pub nickname: String,
    /// The username of the user who created the notification.
    pub username: String,
    /// The hostname of the user who created the notification.
    pub hostname: String,
    /// The channel the notification was created in.
    pub channel: String,
    /// The notification message.
    pub message: String,
    /// The time the notification was created.
    pub created_at: DateTime<Utc>,
}

/// A notification to be inserted into the database.
#[derive(Debug, Clone)]
pub struct NewNotification {
    /// The nickname the notification is aimed at.
    pub target: String,
    /// The nickname of the user creating the notification.
    pub nickname: String,
    /// The username of the user creating the notification.
    pub username: String,
    /// The hostname of the user creating the notification.
    pub hostname: String,
    /// The channel the notification is created in.
    pub channel: String,
    /// The notification message.
    pub message: String,
}
