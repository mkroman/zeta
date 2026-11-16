//! Client for IMDb's internal GraphQL API.
//!
//! The client is split by concern: [`query`] holds the GraphQL documents it sends, [`model`] the
//! response nodes and their mapping into the plugin's data model, and `tests` the
//! recorded-response tests. This module owns the HTTP client, the response envelope, and the
//! public `title`, `person` and `search` operations.

mod model;
mod query;
#[cfg(test)]
mod tests;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tracing::{debug, instrument};

use self::model::{NameData, NameNode, SearchData, TitleData, title_from_node};
use self::query::{GET_NAME_QUERY, GET_TITLE_QUERY, SEARCH_QUERY};
use super::Settings;
use super::error::Error;
use super::model::{Person, SearchResult, Title};
use zeta_http as http;
use zeta_http::HttpConfig;
use zeta_http::RequestError;
use zeta_http::reqwest;
use zeta_http::reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue, ORIGIN, REFERER};

/// GraphQL endpoint of IMDb's internal API.
pub(crate) const GRAPHQL_URL: &str = "https://api.graphql.imdb.com/";

/// A client for IMDb's internal GraphQL API.
pub struct GraphQlClient {
    /// HTTP client for GraphQL requests.
    http: reqwest::Client,
    /// Whether to include adult titles in search results.
    include_adult: bool,
}

impl GraphQlClient {
    /// Creates a new IMDb GraphQL client using the given plugin settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client could not be built, or a configured country or
    /// language is not a valid header value.
    pub fn try_new(settings: &Settings, config: &HttpConfig) -> Result<Self, Error> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(ORIGIN, HeaderValue::from_static("https://www.imdb.com"));
        headers.insert(REFERER, HeaderValue::from_static("https://www.imdb.com/"));
        headers.insert(
            HeaderName::from_static("x-imdb-user-country"),
            HeaderValue::from_str(&settings.user_country)?,
        );
        headers.insert(
            HeaderName::from_static("x-imdb-user-language"),
            HeaderValue::from_str(&settings.user_language)?,
        );

        let http = http::builder(config)
            .default_headers(headers)
            .build()
            .map_err(|error| Error::from(RequestError::from(error)))?;

        Ok(Self {
            http,
            include_adult: settings.include_adult,
        })
    }

    /// Fetches details about the title with the given id (e.g. `tt1375666`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if no title exists with the given id, and request or
    /// deserialization errors as appropriate.
    #[instrument(name = "imdb.title", skip_all, fields(imdb.title.id = %id))]
    pub async fn title(&self, id: &str) -> Result<Title, Error> {
        debug!("fetching title details");

        let data: TitleData = self
            .execute(GET_TITLE_QUERY, json!({ "id": id }), "GetTitle")
            .await?;

        title_from_node(data.title)
    }

    /// Fetches details about the person with the given id (e.g. `nm0186505`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if no person exists with the given id, and request or
    /// deserialization errors as appropriate.
    #[instrument(name = "imdb.person", skip_all, fields(imdb.person.id = %id))]
    pub async fn person(&self, id: &str) -> Result<Person, Error> {
        debug!("fetching person details");

        let data: NameData = self
            .execute(GET_NAME_QUERY, json!({ "id": id }), "GetName")
            .await?;

        data.name
            .filter(NameNode::has_name_text)
            .map(Person::from)
            .ok_or(Error::NotFound)
    }

    /// Searches for titles matching `query`, returning at most `limit` results.
    ///
    /// # Errors
    ///
    /// Returns request or deserialization errors as appropriate.
    #[instrument(name = "imdb.search", skip_all, fields(search.query = %query))]
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, Error> {
        debug!(search.limit = %limit, "searching for titles");

        let data: SearchData = self
            .execute(
                SEARCH_QUERY,
                json!({ "searchTerm": query, "first": limit, "includeAdult": self.include_adult }),
                "Search",
            )
            .await?;

        let results = data.main_search.map_or_default(|search| {
            search
                .edges
                .into_iter()
                .filter_map(|edge| edge.node.entity)
                .map(SearchResult::from)
                .collect()
        });

        Ok(results)
    }

    /// Executes a GraphQL query, returning the `data` payload of the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the response cannot be parsed,
    /// [`Error::GraphQL`] if the API reported any errors, and [`Error::UnexpectedResponse`] if
    /// the response contains no data.
    async fn execute<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: serde_json::Value,
        operation_name: &str,
    ) -> Result<T, Error> {
        let payload = json!({
            "query": query,
            "variables": variables,
            "operationName": operation_name,
        });

        let response: GraphQlResponse<T> =
            http::get_json(self.http.post(GRAPHQL_URL).json(&payload)).await?;

        if let Some(errors) = response.errors.filter(|errors| !errors.is_empty()) {
            let messages = errors
                .into_iter()
                .map(|error| error.message.unwrap_or_else(|| "unknown error".to_string()))
                .collect::<Vec<_>>()
                .join("; ");

            return Err(Error::GraphQL(messages));
        }

        response.data.ok_or(Error::UnexpectedResponse)
    }
}

/// An envelope for GraphQL API responses.
#[derive(Debug, Deserialize)]
struct GraphQlResponse<T> {
    /// The data payload of the query, present when the query succeeded.
    data: Option<T>,
    /// Any errors reported while executing the query.
    errors: Option<Vec<GraphQlError>>,
}

/// An error reported by the GraphQL API.
#[derive(Debug, Deserialize)]
struct GraphQlError {
    /// A human-readable description of the error.
    message: Option<String>,
}
