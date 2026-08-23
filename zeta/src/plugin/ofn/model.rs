use sqlx::{
    prelude::FromRow,
    types::chrono::{DateTime, Utc},
};

#[derive(Debug, FromRow)]
#[allow(dead_code)]
pub struct UrlRecord {
    pub id: i32,
    pub scheme: String,
    pub host: String,
    pub port: i32,
    pub path: Option<String>,
    pub query: Option<String>,
    pub fragment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub nickname: String,
    pub username: String,
    pub hostname: String,
    pub channel: String,
    pub network_id: Option<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct InsertUrlRecord {
    pub scheme: String,
    pub host: String,
    pub port: i32,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
    pub nickname: String,
    pub username: String,
    pub hostname: String,
    pub channel: String,
    pub network_id: String,
}
