//! The data model of the notification plugin.

use sqlx::{
    prelude::FromRow,
    types::chrono::{DateTime, Utc},
};

use zeta_plugin::database::Attribution;

/// A pending notification, as stored in the database.
///
/// The username and hostname columns are written on insert but not part of the row model,
/// which only carries what the plugin reads.
#[derive(Debug, FromRow, Clone)]
pub struct Notification {
    /// The database id of the notification.
    pub id: i32,
    /// The nickname the notification is aimed at.
    pub target: String,
    /// The nickname of the user who created the notification.
    pub nickname: String,
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
    /// The user who created the notification, and the channel it is created in.
    pub attribution: Attribution,
    /// The notification message.
    pub message: String,
}
