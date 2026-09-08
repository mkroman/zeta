//! Client for IMDb's internal GraphQL API.

use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue, ORIGIN, REFERER};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tracing::{debug, error, instrument};

use super::error::Error;
use super::model::{Person, SearchResult, SeriesInfo, Title};
use crate::http;


/// GraphQL endpoint of IMDb's internal API.
pub const GRAPHQL_URL: &str = "https://api.graphql.imdb.com/";

/// Query fetching the details of a single title.
const GET_TITLE_QUERY: &str = r"
query GetTitle($id: ID!) {
  title(id: $id) {
    id
    titleText { text }
    originalTitleText { text }
    titleType { text }
    releaseYear { year endYear }
    ratingsSummary { aggregateRating voteCount }
    plot { plotText { plainText } }
    titleGenres { genres { genre { text } } }
    series {
      series { id titleText { text } }
      displayableEpisodeNumber {
        episodeNumber { episodeNumber }
        displayableSeason { season }
      }
    }
  }
}
";

/// Query fetching the details of a single person.
const GET_NAME_QUERY: &str = r"
query GetName($id: ID!) {
  name(id: $id) {
    id
    nameText { text }
    birthDate { dateComponents { year } }
    deathDate { dateComponents { year } }
    bios(first: 1) {
      edges {
        node {
          text { plainText }
        }
      }
    }
    knownFor(first: 3) {
      edges {
        node {
          ... on NameKnownFor {
            title {
              id
              titleText { text }
              releaseYear { year endYear }
            }
          }
        }
      }
    }
  }
}
";

/// Query searching for titles matching a search term.
const SEARCH_QUERY: &str = r"
query Search($searchTerm: String!, $first: Int!) {
  mainSearch(first: $first, options: {searchTerm: $searchTerm, type: TITLE, includeAdult: true}) {
    edges {
      node {
        entity {
          ... on Title {
            id
            titleText { text }
            originalTitleText { text }
            titleType { text }
            releaseYear { year }
            ratingsSummary { aggregateRating }
          }
        }
      }
    }
  }
}
";


/// A client for IMDb's internal GraphQL API.
pub struct GraphQlClient {
    /// HTTP client for GraphQL requests.
    http: reqwest::Client,
}

impl Default for GraphQlClient {
    fn default() -> Self {
        Self::new().expect("could not build http client")
    }
}

impl GraphQlClient {
    /// Creates a new IMDb GraphQL client.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client could not be built.
    pub fn new() -> Result<Self, Error> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(ORIGIN, HeaderValue::from_static("https://www.imdb.com"));
        headers.insert(REFERER, HeaderValue::from_static("https://www.imdb.com/"));
        headers.insert(
            HeaderName::from_static("x-imdb-user-country"),
            HeaderValue::from_static("US"),
        );
        headers.insert(
            HeaderName::from_static("x-imdb-user-language"),
            HeaderValue::from_static("en-US"),
        );

        let http = http::client::builder()
            .default_headers(headers)
            .build()
            .map_err(Error::Request)?;

        Ok(Self { http })
    }

    /// Fetches details about the title with the given id (e.g. `tt1375666`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if no title exists with the given id, and request or
    /// deserialization errors as appropriate.
    #[instrument(skip(self), fields(%id))]
    pub async fn title(&self, id: &str) -> Result<Title, Error> {
        debug!(%id, "fetching title details");

        let data: TitleData = self
            .execute(GET_TITLE_QUERY, json!({ "id": id }), "GetTitle")
            .await?;

        // Titles that do not exist are returned as an empty node (all fields null) rather than
        // `null`, so a title without any title text is treated as not found.
        data.title
            .filter(|title| title.title_text.is_some() || title.original_title_text.is_some())
            .map(Title::from)
            .ok_or(Error::NotFound)
    }

    /// Fetches details about the person with the given id (e.g. `nm0186505`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if no person exists with the given id, and request or
    /// deserialization errors as appropriate.
    #[instrument(skip(self), fields(%id))]
    pub async fn person(&self, id: &str) -> Result<Person, Error> {
        debug!(%id, "fetching person details");

        let data: NameData = self
            .execute(GET_NAME_QUERY, json!({ "id": id }), "GetName")
            .await?;

        data.name
            .filter(|name| name.name_text.is_some())
            .map(Person::from)
            .ok_or(Error::NotFound)
    }

    /// Searches for titles matching `query`, returning at most `limit` results.
    ///
    /// # Errors
    ///
    /// Returns request or deserialization errors as appropriate.
    #[instrument(skip(self), fields(%query))]
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, Error> {
        debug!(%query, %limit, "searching for titles");

        let data: SearchData = self
            .execute(
                SEARCH_QUERY,
                json!({ "searchTerm": query, "first": limit }),
                "Search",
            )
            .await?;

        let results = data
            .main_search
            .map(|search| {
                search
                    .edges
                    .into_iter()
                    .filter_map(|edge| edge.node.entity)
                    .map(SearchResult::from)
                    .collect()
            })
            .unwrap_or_default();

        Ok(results)
    }

    /// Executes a GraphQL query, returning the `data` payload of the response.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Request`] if the request failed, [`Error::Deserialize`] if the response
    /// could not be parsed, [`Error::GraphQL`] if the API reported any errors, and
    /// [`Error::UnexpectedResponse`] if the response contains no data.
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

        let response = self
            .http
            .post(GRAPHQL_URL)
            .json(&payload)
            .send()
            .await
            .map_err(Error::Request)?
            .error_for_status()
            .map_err(Error::Request)?;

        let text = response.text().await.map_err(Error::Request)?;
        let response: GraphQlResponse<T> = decode_response(&text)?;

        if let Some(errors) = response.errors.filter(|errors| !errors.is_empty()) {
            let messages = errors
                .into_iter()
                .map(|error| error.message.unwrap_or_else(|| "unknown error".to_string()))
                .collect::<Vec<_>>()
                .join("; ");
            error!(%messages, "graphql query failed");

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

/// Deserializes a GraphQL response body into [`GraphQlResponse`].
fn decode_response<T: DeserializeOwned>(text: &str) -> Result<GraphQlResponse<T>, Error> {
    let deserializer = &mut serde_json::Deserializer::from_str(text);

    serde_path_to_error::deserialize(deserializer)
        .inspect_err(|err| error!(?err, %text, "could not deserialize response"))
        .map_err(Error::Deserialize)
}

/// A GraphQL text field (e.g. `titleText { text }`).
#[derive(Debug, Deserialize)]
struct TextField {
    text: String,
}

/// A GraphQL release year field.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseYear {
    year: Option<i32>,
    end_year: Option<i32>,
}

/// A GraphQL ratings summary.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RatingsSummary {
    aggregate_rating: Option<f32>,
    vote_count: Option<u64>,
}

/// A GraphQL plain text field (e.g. `plotText { plainText }`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlainTextField {
    plain_text: String,
}

/// A GraphQL plot field.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plot {
    plot_text: Option<PlainTextField>,
}

/// A GraphQL title genres field.
#[derive(Debug, Deserialize)]
struct TitleGenres {
    genres: Vec<Genre>,
}

/// A single genre entry.
#[derive(Debug, Deserialize)]
struct Genre {
    genre: Option<TextField>,
}

/// A GraphQL title node.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TitleNode {
    id: String,
    title_text: Option<TextField>,
    original_title_text: Option<TextField>,
    title_type: Option<TextField>,
    release_year: Option<ReleaseYear>,
    ratings_summary: Option<RatingsSummary>,
    plot: Option<Plot>,
    title_genres: Option<TitleGenres>,
    series: Option<SeriesField>,
}

impl From<TitleNode> for Title {
    fn from(node: TitleNode) -> Self {
        let genres = node
            .title_genres
            .map(|genres| {
                genres
                    .genres
                    .into_iter()
                    .filter_map(|genre| genre.genre.map(|genre| genre.text))
                    .collect()
            })
            .unwrap_or_default();
        let ratings = node.ratings_summary;
        let series = node.series.and_then(SeriesField::into_series_info);

        Title {
            id: node.id,
            title: node.title_text.map(|text| text.text),
            original_title: node.original_title_text.map(|text| text.text),
            title_type: node.title_type.map(|text| text.text),
            year: node.release_year.as_ref().and_then(|year| year.year),
            end_year: node.release_year.and_then(|year| year.end_year),
            rating: ratings
                .as_ref()
                .and_then(|ratings| ratings.aggregate_rating),
            votes: ratings.and_then(|ratings| ratings.vote_count),
            plot: node
                .plot
                .and_then(|plot| plot.plot_text.map(|text| text.plain_text)),
            genres,
            series,
        }
    }
}

/// A GraphQL series field of an episode.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesField {
    series: Option<SeriesRef>,
    displayable_episode_number: Option<DisplayableEpisodeNumber>,
}

impl SeriesField {
    /// Converts the series field into a [`SeriesInfo`], requiring the series reference.
    fn into_series_info(self) -> Option<SeriesInfo> {
        let series = self.series?;
        let numbers = self.displayable_episode_number;

        Some(SeriesInfo {
            id: series.id,
            title: series.title_text.map(|text| text.text),
            season: numbers
                .as_ref()
                .and_then(|numbers| numbers.displayable_season.as_ref())
                .and_then(|season| season.season.clone()),
            number: numbers
                .and_then(|numbers| numbers.episode_number)
                .and_then(|number| number.episode_number),
        })
    }
}

/// A reference to the series an episode belongs to.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesRef {
    id: String,
    title_text: Option<TextField>,
}

/// The displayed season and episode numbers of an episode.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisplayableEpisodeNumber {
    episode_number: Option<EpisodeNumberField>,
    displayable_season: Option<SeasonField>,
}

/// A GraphQL episode number field.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeNumberField {
    episode_number: Option<String>,
}

/// A GraphQL season field.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeasonField {
    season: Option<String>,
}

/// The data payload of a `GetTitle` query response.
#[derive(Debug, Deserialize)]
struct TitleData {
    title: Option<TitleNode>,
}

/// The data payload of a `GetName` query response.
#[derive(Debug, Deserialize)]
struct NameData {
    name: Option<NameNode>,
}

/// A GraphQL person node.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameNode {
    id: String,
    name_text: Option<TextField>,
    birth_date: Option<DateField>,
    death_date: Option<DateField>,
    bios: Option<Bios>,
    known_for: Option<KnownFor>,
}

impl From<NameNode> for Person {
    fn from(node: NameNode) -> Self {
        let bio = node.bios.and_then(|bios| {
            bios.edges
                .into_iter()
                .filter_map(|edge| edge.node.text)
                .map(|text| text.plain_text)
                .next()
        });
        let known_for = node
            .known_for
            .map(|known| {
                known
                    .edges
                    .into_iter()
                    .filter_map(|edge| edge.node.title)
                    .map(Title::from)
                    .collect()
            })
            .unwrap_or_default();

        Person {
            id: node.id,
            name: node.name_text.map(|text| text.text),
            birth_year: node
                .birth_date
                .and_then(|date| date.date_components)
                .and_then(|components| components.year),
            death_year: node
                .death_date
                .and_then(|date| date.date_components)
                .and_then(|components| components.year),
            bio,
            known_for,
        }
    }
}

/// A GraphQL date field.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DateField {
    date_components: Option<DateYear>,
}

/// The year component of a date.
#[derive(Debug, Deserialize)]
struct DateYear {
    year: Option<i32>,
}

/// A GraphQL bios field.
#[derive(Debug, Deserialize)]
struct Bios {
    edges: Vec<Edge<BioNode>>,
}

/// A node in a list of biographies.
#[derive(Debug, Deserialize)]
struct BioNode {
    text: Option<PlainTextField>,
}

/// A GraphQL known-for field.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnownFor {
    edges: Vec<Edge<KnownForNode>>,
}

/// A node in a known-for list.
#[derive(Debug, Deserialize)]
struct KnownForNode {
    title: Option<KnownForTitle>,
}

/// A title a person is known for.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnownForTitle {
    id: String,
    title_text: Option<TextField>,
    release_year: Option<ReleaseYear>,
}

impl From<KnownForTitle> for Title {
    fn from(node: KnownForTitle) -> Self {
        Title {
            id: node.id,
            title: node.title_text.map(|text| text.text),
            year: node.release_year.as_ref().and_then(|year| year.year),
            end_year: node.release_year.and_then(|year| year.end_year),
            ..Title::default()
        }
    }
}

/// The data payload of a `Search` query response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchData {
    main_search: Option<MainSearch>,
}

/// A search result page.
#[derive(Debug, Deserialize)]
struct MainSearch {
    edges: Vec<Edge<SearchNode>>,
}

/// An edge in a paginated list.
#[derive(Debug, Deserialize)]
struct Edge<T> {
    node: T,
}

/// A node in a search result page.
#[derive(Debug, Deserialize)]
struct SearchNode {
    /// The matched entity, absent for non-title entities.
    entity: Option<EntityNode>,
}

/// A matched title entity.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntityNode {
    id: String,
    title_text: Option<TextField>,
    release_year: Option<ReleaseYear>,
}

impl From<EntityNode> for SearchResult {
    fn from(node: EntityNode) -> Self {
        SearchResult {
            id: node.id,
            title: node.title_text.map(|text| text.text),
            year: node.release_year.and_then(|year| year.year),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_get_title_response() {
        let text = r#"{"data":{"title":{"id":"tt14663588","titleText":{"text":"Peggle Nights"},"originalTitleText":{"text":"Peggle Nights"},"titleType":{"text":"Video Game"},"releaseYear":{"year":2008,"endYear":null},"ratingsSummary":{"aggregateRating":7.6,"voteCount":53},"plot":{"plotText":{"plainText":"Downloadable follow-up to the original \"Peggle (2007)\"."}},"titleGenres":{"genres":[{"genre":{"text":"Action"}},{"genre":{"text":"Adventure"}},{"genre":{"text":"Family"}}]}}}}"#;

        let response: GraphQlResponse<TitleData> = decode_response(text).unwrap();
        let title = Title::from(response.data.unwrap().title.unwrap());

        assert_eq!(title.id, "tt14663588");
        assert_eq!(title.title.as_deref(), Some("Peggle Nights"));
        assert_eq!(title.title_type.as_deref(), Some("Video Game"));
        assert_eq!(title.year, Some(2008));
        assert_eq!(title.end_year, None);
        assert_eq!(title.rating, Some(7.6));
        assert_eq!(title.votes, Some(53));
        assert!(title.plot.as_deref().unwrap().starts_with("Downloadable"));
        assert_eq!(title.genres, ["Action", "Adventure", "Family"]);
    }

    #[test]
    fn decodes_search_response() {
        let text = r#"{"data":{"mainSearch":{"edges":[{"node":{"entity":{"id":"tt14663588","titleText":{"text":"Peggle Nights"},"releaseYear":{"year":2008}}}}]}}}"#;

        let response: GraphQlResponse<SearchData> = decode_response(text).unwrap();
        let data = response.data.unwrap();
        let results: Vec<SearchResult> = data
            .main_search
            .unwrap()
            .edges
            .into_iter()
            .filter_map(|edge| edge.node.entity)
            .map(SearchResult::from)
            .collect();

        assert_eq!(
            results,
            vec![SearchResult {
                id: "tt14663588".to_string(),
                title: Some("Peggle Nights".to_string()),
                year: Some(2008),
            }]
        );
    }

    #[test]
    fn decodes_null_title_as_not_found() {
        let text = r#"{"data":{"title":null}}"#;
        let response: GraphQlResponse<TitleData> = decode_response(text).unwrap();

        assert!(response.data.unwrap().title.is_none());
    }

    #[test]
    fn decodes_episode_response_with_series_info() {
        let text = r#"{"data":{"title":{"id":"tt0959621","titleText":{"text":"Pilot"},"titleType":{"text":"TV Episode"},"releaseYear":{"year":2008,"endYear":null},"series":{"series":{"id":"tt0903747","titleText":{"text":"Breaking Bad"}},"displayableEpisodeNumber":{"episodeNumber":{"episodeNumber":"1"},"displayableSeason":{"season":"1"}}}}}}"#;

        let response: GraphQlResponse<TitleData> = decode_response(text).unwrap();
        let title = Title::from(response.data.unwrap().title.unwrap());

        assert!(title.is_episode());
        let series = title.series.expect("series info");
        assert_eq!(series.id, "tt0903747");
        assert_eq!(series.title.as_deref(), Some("Breaking Bad"));
        assert_eq!(series.season.as_deref(), Some("1"));
        assert_eq!(series.number.as_deref(), Some("1"));
        assert_eq!(series.episode_marker().as_deref(), Some("S01E01"));
    }

    #[test]
    fn decodes_name_response() {
        let text = r#"{"data":{"name":{"id":"nm0186505","nameText":{"text":"Bryan Cranston"},"birthDate":{"dateComponents":{"year":1956}},"deathDate":null,"bios":{"edges":[{"node":{"text":{"plainText":"Bryan Lee Cranston was born on March 7, 1956."}}}]},"knownFor":{"edges":[{"node":{"title":{"id":"tt0903747","titleText":{"text":"Breaking Bad"},"releaseYear":{"year":2008,"endYear":2013}}}}]}}}}"#;

        let response: GraphQlResponse<NameData> = decode_response(text).unwrap();
        let person = Person::from(response.data.unwrap().name.unwrap());

        assert_eq!(person.id, "nm0186505");
        assert_eq!(person.name.as_deref(), Some("Bryan Cranston"));
        assert_eq!(person.birth_year, Some(1956));
        assert_eq!(person.death_year, None);
        assert_eq!(
            person.bio.as_deref(),
            Some("Bryan Lee Cranston was born on March 7, 1956.")
        );
        assert_eq!(person.known_for.len(), 1);
        assert_eq!(person.known_for[0].title.as_deref(), Some("Breaking Bad"));
        assert_eq!(person.known_for[0].end_year, Some(2013));
    }
}
