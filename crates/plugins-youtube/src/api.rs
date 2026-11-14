//! Request helpers for the YouTube Data API.

use std::collections::HashMap;

use tracing::debug;

use super::model::{
    CategoriesResponse, Category, Search, SearchListResponse, Video, VideosResponse,
};
use super::{Error, YouTube};
use zeta_http as http;
use zeta_http::reqwest;

/// The endpoint videos are searched against.
const SEARCH_URL: &str = "https://www.googleapis.com/youtube/v3/search";
/// The endpoint video details are fetched from.
const VIDEOS_URL: &str = "https://www.googleapis.com/youtube/v3/videos";
/// The endpoint video categories are fetched from.
const VIDEO_CATEGORIES_URL: &str = "https://www.googleapis.com/youtube/v3/videoCategories";

impl YouTube {
    /// Searches for videos using the given query.
    pub(super) async fn search(&self, query: &str) -> Result<Vec<Search>, Error> {
        debug!(search.query = %query, "searching for videos");

        let params = [
            ("q", query),
            ("key", &self.api_key),
            ("part", "snippet"),
            ("type", "video"),
            ("safeSearch", self.safe_search.as_str()),
        ];

        let request = self.client.get(SEARCH_URL).query(&params);

        let result: SearchListResponse = http::get_json(request).await?;

        let items = result.items;

        debug!(?items, "returning items");

        Ok(items)
    }

    /// Fetches metadata for a YouTube video using its video ID.
    ///
    /// Returns `Err(Error::NoResults)` if no video is found with the given ID.
    pub(super) async fn get_video(&self, video_id: &str) -> Result<Video, Error> {
        debug!(media.id = %video_id, "fetching video metadata");

        let params = [
            ("id", video_id),
            ("key", &self.api_key),
            (
                "part",
                "snippet,statistics,contentDetails,liveStreamingDetails",
            ),
        ];
        let request = self.client.get(VIDEOS_URL).query(&params);
        let list: VideosResponse = http::get_json(request).await?;

        debug!("fetched metadata for video");

        list.items.into_iter().next().ok_or(Error::NoResults)
    }
}

/// Fetches the video categories map from the API, keyed by category id.
///
/// # Errors
///
/// Returns [`Error::Api`] if the request failed or the response could not be parsed, and
/// [`Error::NoResults`] if the API returned no categories.
pub async fn fetch_video_categories(
    client: &reqwest::Client,
    api_key: &str,
    region_code: &str,
) -> Result<HashMap<String, Category>, Error> {
    debug!("fetching video categories");

    let params = [
        ("key", api_key),
        ("part", "snippet"),
        ("regionCode", region_code),
    ];
    let request = client.get(VIDEO_CATEGORIES_URL).query(&params);
    let list: CategoriesResponse = http::get_json(request).await?;

    debug!("fetched video category list");

    let map: HashMap<String, Category> =
        list.items.into_iter().map(|c| (c.id.clone(), c)).collect();

    if map.is_empty() {
        Err(Error::NoResults)
    } else {
        Ok(map)
    }
}
