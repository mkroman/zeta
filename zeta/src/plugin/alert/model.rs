//! The data model of the alert plugin.

use sqlx::{
    prelude::FromRow,
    types::chrono::{DateTime, Utc},
};

/// A scheduled alert, as stored in the database.
// Some fields are only populated from the database and never read.
#[derive(Debug, FromRow, Clone)]
#[allow(dead_code)]
pub struct Alert {
    /// The database id of the alert.
    pub id: i32,
    /// The nickname of the user who created the alert.
    pub nickname: String,
    /// The username of the user who created the alert.
    pub username: String,
    /// The hostname of the user who created the alert.
    pub hostname: String,
    /// The channel the alert is delivered in.
    pub channel: String,
    /// The alert message.
    pub message: String,
    /// The time the alert is scheduled for.
    pub time: DateTime<Utc>,
    /// The time the alert was created.
    pub created_at: DateTime<Utc>,
}

/// An alert to be inserted into the database.
#[derive(Debug, Clone)]
pub struct NewAlert {
    /// The nickname of the user creating the alert.
    pub nickname: String,
    /// The username of the user creating the alert.
    pub username: String,
    /// The hostname of the user creating the alert.
    pub hostname: String,
    /// The channel the alert is delivered in.
    pub channel: String,
    /// The alert message.
    pub message: String,
    /// The time the alert is scheduled for.
    pub time: DateTime<Utc>,
}
