use sqlx::{
    prelude::FromRow,
    types::chrono::{DateTime, Utc},
};

/// A recorded URL, as the repost reply uses it.
#[derive(Debug, FromRow)]
pub struct UrlRecord {
    pub created_at: DateTime<Utc>,
    pub nickname: String,
}

/// A recorded YouTube video, as the repost reply uses it.
#[derive(Debug, FromRow)]
pub struct YouTubeRecord {
    pub created_at: DateTime<Utc>,
    pub nickname: String,
}
