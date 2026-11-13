//! The YouTube Data API response models and the cached video category types.

use serde::Deserialize;

/// Basic details about the video, such as its title and category.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    /// The video title, possibly containing HTML entities.
    pub title: String,
    /// The title of the channel that uploaded the video.
    pub channel_title: String,
    /// The video category id, resolvable through the video categories map.
    pub category_id: String,
    /// Whether the video is a `"live"`, `"upcoming"`, or regular broadcast.
    pub live_broadcast_content: String,
}

/// Statistics about a video.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Statistics {
    /// The number of times the video has been viewed, as a decimal string.
    pub view_count: String,
}

/// Details about the content of a video, such as its duration.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentDetails {
    /// The video duration as an ISO 8601 duration string (e.g. `PT4M13S`).
    pub duration: String,
}

/// Details about a live stream.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStreamingDetails {
    /// The number of concurrent viewers, present while the stream is live.
    pub concurrent_viewers: Option<String>,
}

/// A YouTube video.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Video {
    /// The video's basic details, present in the `snippet` API part.
    pub snippet: Option<Snippet>,
    /// The video's statistics, present in the `statistics` API part.
    pub statistics: Option<Statistics>,
    /// The video's content details, present in the `contentDetails` API part.
    pub content_details: Option<ContentDetails>,
    /// The video's live streaming details, present for live and upcoming streams.
    pub live_streaming_details: Option<LiveStreamingDetails>,
}

/// Search Result.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Search {
    /// The id of the matched resource, typed by its kind.
    pub id: SearchId,
    /// The matched resource's basic details.
    pub snippet: SearchSnippet,
}

/// The id of a search result, typed by the kind of resource the search matched.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchId {
    /// The video id, present for video results.
    pub video_id: Option<String>,
}

/// The snippet of a search result.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnippet {
    /// The matched resource's title, possibly containing HTML entities.
    pub title: String,
}

/// Details about a video category.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySnippet {
    /// The category title, e.g. "Music".
    pub title: String,
}

/// A video category result.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    /// The category id.
    pub id: String,
    /// The category's details.
    pub snippet: CategorySnippet,
}

/// Generic response type for list results.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ApiListResponse<R> {
    /// The list items.
    pub items: Vec<R>,
}

/// Response with a list of YouTube videos.
pub type VideosResponse = ApiListResponse<Video>;

/// Response with a list of YouTube video categories.
pub type CategoriesResponse = ApiListResponse<Category>;

/// Response with a list of YouTube search results.
pub type SearchListResponse = ApiListResponse<Search>;
