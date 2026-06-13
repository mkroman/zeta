use std::collections::HashMap;

use serde::Deserialize;
use time::OffsetDateTime;

/// Basic details about the video, such as its title, description, and category.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct Snippet {
    pub title: String,
    pub description: String,
    pub channel_title: String,
    pub category_id: String,
}

/// Statistics about a video.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct Statistics {
    pub view_count: String,
}

/// A YouTube video.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct Video {
    pub kind: String,
    pub etag: String,
    pub id: String,
    pub snippet: Option<Snippet>,
    pub statistics: Option<Statistics>,
}

/// Search Result.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct Search {
    pub kind: String,
    pub etag: String,
    pub id: SearchId,
    pub snippet: SearchSnippet,
}

// TODO: rework this so it uses an enum
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct SearchId {
    pub kind: String,
    pub video_id: Option<String>,
    pub channel_id: Option<String>,
    pub playlist_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct SearchSnippet {
    pub title: String,
    pub description: String,
    pub channel_id: String,
    pub channel_title: String,
    pub thumbnails: HashMap<String, SearchSnippetThumbnail>,
    #[serde(with = "time::serde::rfc3339")]
    pub published_at: OffsetDateTime,
    pub live_broadcast_content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct SearchSnippetThumbnail {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

/// Details about a video category.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct CategorySnippet {
    pub channel_id: String,
    pub title: String,
    pub assignable: bool,
}

/// A video category result.
#[derive(Clone, Debug, Deserialize)]
#[allow(unused)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub kind: String,
    pub etag: String,
    pub id: String,
    pub snippet: CategorySnippet,
}

/// Generic response type for list results.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct ApiListResponse<R> {
    pub kind: String,
    pub etag: String,
    pub items: Vec<R>,
}

/// Response with a list of YouTube videos.
pub type VideosResponse = ApiListResponse<Video>;

/// Response with a list of YouTube video categories.
pub type CategoriesResponse = ApiListResponse<Category>;

pub type SearchListResponse = ApiListResponse<Search>;
