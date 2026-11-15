//! GraphQL response nodes and their mapping into the plugin's data model.
//!
//! The nodes mirror the field selection of the queries in [`super::query`], and the `From`
//! conversions fill the [`Title`], [`Person`] and [`SearchResult`] types the plugin formats and
//! replies with. The `*Data` envelopes are the `data` payload of each query response.

use serde::Deserialize;

use crate::error::Error;
use crate::model::{Person, SearchResult, SeriesInfo, Title};

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
pub(super) struct TitleNode {
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
        let genres = node.title_genres.map_or_default(|genres| {
            genres
                .genres
                .into_iter()
                .filter_map(|genre| genre.genre.map(|genre| genre.text))
                .collect()
        });
        let ratings = node.ratings_summary;
        let series = node.series.and_then(SeriesField::into_series_info);

        Self {
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
pub(super) struct TitleData {
    pub(super) title: Option<TitleNode>,
}

/// Converts a title node into a [`Title`].
///
/// Titles that do not exist are returned as an empty node (all fields null) rather than `null`,
/// so a title without any title text is treated as not found.
pub(super) fn title_from_node(node: Option<TitleNode>) -> Result<Title, Error> {
    node.filter(|title| title.title_text.is_some() || title.original_title_text.is_some())
        .map(Title::from)
        .ok_or(Error::NotFound)
}

/// The data payload of a `GetName` query response.
#[derive(Debug, Deserialize)]
pub(super) struct NameData {
    pub(super) name: Option<NameNode>,
}

/// A GraphQL person node.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NameNode {
    id: String,
    name_text: Option<TextField>,
    birth_date: Option<DateField>,
    death_date: Option<DateField>,
    bios: Option<Bios>,
    known_for: Option<KnownFor>,
}

impl NameNode {
    /// Checks whether the node carries a display name, which marks an existing person.
    pub(super) const fn has_name_text(&self) -> bool {
        self.name_text.is_some()
    }
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
        let known_for = node.known_for.map_or_default(|known| {
            known
                .edges
                .into_iter()
                .filter_map(|edge| edge.node.title)
                .map(Title::from)
                .collect()
        });

        Self {
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
        Self {
            id: node.id,
            title: node.title_text.map(|text| text.text),
            year: node.release_year.as_ref().and_then(|year| year.year),
            end_year: node.release_year.and_then(|year| year.end_year),
            ..Self::default()
        }
    }
}

/// The data payload of a `Search` query response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SearchData {
    pub(super) main_search: Option<MainSearch>,
}

/// A search result page.
#[derive(Debug, Deserialize)]
pub(super) struct MainSearch {
    pub(super) edges: Vec<Edge<SearchNode>>,
}

/// An edge in a paginated list.
#[derive(Debug, Deserialize)]
pub(super) struct Edge<T> {
    pub(super) node: T,
}

/// A node in a search result page.
#[derive(Debug, Deserialize)]
pub(super) struct SearchNode {
    /// The matched entity, absent for non-title entities.
    pub(super) entity: Option<EntityNode>,
}

/// A matched title entity.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EntityNode {
    id: String,
    title_text: Option<TextField>,
    release_year: Option<ReleaseYear>,
}

impl From<EntityNode> for SearchResult {
    fn from(node: EntityNode) -> Self {
        Self {
            id: node.id,
            title: node.title_text.map(|text| text.text),
            year: node.release_year.and_then(|year| year.year),
        }
    }
}
