use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::debug;

use super::types::{
    CategoriesResponse, Category, Search, SearchListResponse, Video, VideosResponse,
};
use super::{Error, YouTube};

/// YouTube Data API v3 base endpoint URL.
pub const BASE_URL: &str = "https://www.googleapis.com/youtube/v3";

impl YouTube {
    /// Fetches video categories.
    async fn video_categories(&self) -> Result<HashMap<String, Category>, Error> {
        debug!("fetching video categories");

        let params = [
            ("key", self.api_key.as_str()),
            ("part", "snippet"),
            ("regionCode", "US"),
        ];
        let request = self
            .client
            .get(format!("{BASE_URL}/videoCategories"))
            .query(&params);
        let response = request
            .send()
            .await
            .map_err(|_| Error::InvalidResponse)?
            .error_for_status()?;
        let list: CategoriesResponse = response.json().await?;

        debug!("fetched video category list");

        let map: HashMap<String, Category> =
            list.items.into_iter().map(|c| (c.id.clone(), c)).collect();

        if map.is_empty() {
            Err(Error::NoResults)
        } else {
            Ok(map)
        }
    }

    pub(super) async fn cached_video_categories(
        &self,
    ) -> Result<Arc<HashMap<String, Category>>, Error> {
        let categories_updated_at = *self.video_categories_updated_at.read().await;
        if let Some(instant) = categories_updated_at {
            debug!("using cached video categories");

            if instant.elapsed() < Duration::from_mins(30) {
                let vc = self.video_categories.read().await;

                return Ok(vc.clone());
            }
        }

        debug!("refreshing cached video categories");
        let new_categories = self.video_categories().await?;
        let categories_arc = Arc::new(new_categories);

        {
            let mut categories_guard = self.video_categories.write().await;
            *categories_guard = categories_arc.clone();
        }
        {
            let mut updated_at_guard = self.video_categories_updated_at.write().await;
            *updated_at_guard = Some(Instant::now());
        }

        let vc = self.video_categories.read().await;
        Ok(vc.clone())
    }

    /// Searches for videos using the given query.
    pub(super) async fn search(&self, query: &str) -> Result<Vec<Search>, Error> {
        debug!(%query, "searching for videos");

        let params = [
            ("q", query),
            ("key", &self.api_key),
            ("part", "snippet"),
            ("type", "video"),
            ("safeSearch", "none"),
        ];

        debug!(?params, "searching for videos");

        let request = self.client.get(format!("{BASE_URL}/search")).query(&params);
        let response = request.send().await.map_err(Error::Request)?;

        match response.error_for_status() {
            Ok(response) => {
                debug!("response is ok, parsing as json");
                let text = response.text().await.map_err(Error::Request)?;
                let result: SearchListResponse =
                    crate::utils::parse_json(&text).map_err(Error::Deserialize)?;
                let items = result.items;

                debug!(?items, "returning items");

                Ok(items)
            }
            Err(err) => Err(Error::Request(err)),
        }
    }

    /// Fetches metadata for a YouTube video using its video ID.
    ///
    /// Returns `Err(Error::NoResults)` if no video is found with the given ID.
    pub(super) async fn get_video(&self, video_id: &str) -> Result<Video, Error> {
        debug!(%video_id, "fetching video metadata");

        let params = [
            ("id", video_id),
            ("key", &self.api_key),
            ("part", "snippet,statistics,liveStreamingDetails"),
        ];
        let request = self.client.get(format!("{BASE_URL}/videos")).query(&params);
        let response = request
            .send()
            .await
            .map_err(|_| Error::InvalidResponse)?
            .error_for_status()?;
        let list: VideosResponse = response.json().await?;
        debug!("fetched metadata for video");

        if let Some(video) = list.items.first() {
            return Ok(video.clone());
        }

        Err(Error::NoResults)
    }
}
