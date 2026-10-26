//! The data model of the OFN plugin.

use sqlx::{
    prelude::FromRow,
    types::chrono::{DateTime, Utc},
};

/// A URL record, as stored in the database.
#[derive(Debug, FromRow)]
#[allow(dead_code)]
pub struct UrlRecord {
    /// The database id of the record.
    pub id: i32,
    /// The scheme of the posted URL.
    pub scheme: String,
    /// The host of the posted URL.
    pub host: String,
    /// The port of the posted URL.
    pub port: Option<i32>,
    /// The path of the posted URL.
    pub path: Option<String>,
    /// The query of the posted URL.
    pub query: Option<String>,
    /// The fragment of the posted URL.
    pub fragment: Option<String>,
    /// When the URL was first posted.
    pub created_at: DateTime<Utc>,
    /// The nickname of the user who posted the URL.
    pub nickname: String,
    /// The username of the user who posted the URL.
    pub username: String,
    /// The hostname of the user who posted the URL.
    pub hostname: String,
    /// The channel the URL was posted in.
    pub channel: String,
    /// The identifier of the network the URL was posted on.
    pub network_id: Option<String>,
}

/// A YouTube video record, as stored in the database.
#[derive(Debug, FromRow)]
#[allow(dead_code)]
pub struct YouTubeRecord {
    /// The database id of the record.
    pub id: i32,
    /// The video id of the posted YouTube video.
    pub video_id: String,
    /// When the video was first posted.
    pub created_at: DateTime<Utc>,
    /// The nickname of the user who posted the video.
    pub nickname: String,
    /// The username of the user who posted the video.
    pub username: String,
    /// The hostname of the user who posted the video.
    pub hostname: String,
    /// The channel the video was posted in.
    pub channel: String,
    /// The identifier of the network the video was posted on.
    pub network_id: Option<String>,
}

/// A URL record to be inserted into the database.
#[derive(Debug)]
#[allow(dead_code)]
pub struct InsertUrlRecord {
    /// The scheme of the posted URL.
    pub scheme: String,
    /// The host of the posted URL.
    pub host: String,
    /// The port of the posted URL.
    pub port: Option<i32>,
    /// The path of the posted URL.
    pub path: String,
    /// The query of the posted URL.
    pub query: Option<String>,
    /// The fragment of the posted URL.
    pub fragment: Option<String>,
    /// The nickname of the user who posted the URL.
    pub nickname: String,
    /// The username of the user who posted the URL.
    pub username: String,
    /// The hostname of the user who posted the URL.
    pub hostname: String,
    /// The channel the URL was posted in.
    pub channel: String,
    /// The identifier of the network the URL was posted on.
    pub network_id: String,
}

/// A YouTube video record to be inserted into the database.
#[derive(Debug)]
#[allow(dead_code)]
pub struct InsertYouTubeRecord {
    /// The video id of the posted YouTube video.
    pub video_id: String,
    /// The nickname of the user who posted the video.
    pub nickname: String,
    /// The username of the user who posted the video.
    pub username: String,
    /// The hostname of the user who posted the video.
    pub hostname: String,
    /// The channel the video was posted in.
    pub channel: String,
    /// The identifier of the network the video was posted on.
    pub network_id: String,
}

/// Statistics for database rows.
#[derive(Debug)]
#[allow(clippy::struct_field_names)]
pub struct Statistics {
    /// The number of URLs in total.
    pub num_urls: i64,
    /// The number of URLs that were added today.
    pub num_urls_today: i64,
    /// The number of YouTube videos in total.
    pub num_yt_ids: i64,
    /// The number of YouTube videos that have been added today.
    pub num_yt_ids_today: i64,
}
