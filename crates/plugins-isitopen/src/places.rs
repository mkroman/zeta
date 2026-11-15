//! The Google Places text search and place details client.

use serde::Deserialize;
use tracing::{debug, warn};

use super::model::PlaceDetails;
use super::{Error, IsItOpen};
use zeta_http as http;
use zeta_plugin::irc::unstyle_text;

/// The place-text-search endpoint place ids are resolved against.
const TEXTSEARCH_URL: &str = "https://maps.googleapis.com/maps/api/place/textsearch/json";

/// The place-details endpoint place information is fetched from.
const DETAILS_URL: &str = "https://maps.googleapis.com/maps/api/place/details/json";

/// Response from the Text Search API.
#[derive(Debug, Deserialize)]
struct PlaceSearchResponse {
    results: Vec<PlaceSearchResult>,
    status: String,
}

/// A single result from a place search.
#[derive(Debug, Deserialize)]
struct PlaceSearchResult {
    place_id: String,
}

/// Response from the Place Details API.
#[derive(Debug, Deserialize)]
struct PlaceDetailsResponse {
    result: PlaceDetails,
    status: String,
}

impl IsItOpen {
    /// Performs a two-step search: first finding the Place ID, then fetching details.
    pub(super) async fn find_place(&self, query: &str) -> Result<PlaceDetails, Error> {
        debug!(search.query = %query, "searching for place");

        let params = [("query", query), ("key", &self.api_key)];

        let search_res: PlaceSearchResponse =
            http::get_json(self.client.get(TEXTSEARCH_URL).query(&params)).await?;

        if search_res.status != "OK" && search_res.status != "ZERO_RESULTS" {
            warn!(
                places.status = %search_res.status,
                "the place search returned an error status"
            );

            return Err(Error::Places);
        }

        let place_id = search_res
            .results
            .first()
            .ok_or(Error::NotFound)?
            .place_id
            .clone();

        debug!(isitopen.place.id = %place_id, "fetching place details");

        let details_params = [("placeid", &place_id), ("key", &self.api_key)];

        let details_res: PlaceDetailsResponse =
            http::get_json(self.client.get(DETAILS_URL).query(&details_params)).await?;

        if details_res.status != "OK" {
            warn!(
                places.status = %details_res.status,
                "the place details returned an error status"
            );

            return Err(Error::Places);
        }

        // Google's place name is third-party text: strip formatting and line breaks once here
        // so every formatter can interpolate it directly.
        let mut place = details_res.result;
        place.name = unstyle_text(&place.name).into_owned();

        Ok(place)
    }
}
