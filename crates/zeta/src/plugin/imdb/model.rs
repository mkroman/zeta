//! The data model of the IMDb resources printed by the plugin.

/// The series an episode belongs to.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct SeriesInfo {
    /// The unique id of the series (e.g. `tt0903747`).
    pub id: String,
    /// The display title of the series.
    pub title: Option<String>,
    /// The season number of the episode (as displayed by IMDb).
    pub season: Option<String>,
    /// The episode number within the season (as displayed by IMDb).
    pub number: Option<String>,
}

impl SeriesInfo {
    /// Formats the season and episode marker of the episode (e.g. `S01E05`).
    ///
    /// Returns `None` when neither the season nor the episode number is known.
    #[must_use]
    pub fn episode_marker(&self) -> Option<String> {
        match (&self.season, &self.number) {
            (Some(season), Some(number)) => Some(format!("S{season:0>2}E{number:0>2}")),
            (Some(season), None) => Some(format!("S{season:0>2}")),
            (None, Some(number)) => Some(format!("E{number:0>2}")),
            (None, None) => None,
        }
    }
}

/// Details about a title.
#[derive(Debug, Clone, Default, PartialEq)]
#[allow(clippy::struct_field_names)]
pub struct Title {
    /// The unique title id (e.g. `tt1375666`).
    pub id: String,
    /// The display title.
    pub title: Option<String>,
    /// The title in its original language.
    pub original_title: Option<String>,
    /// The type of the title (e.g. "Movie", "TV Series", "Video Game").
    pub title_type: Option<String>,
    /// The year of the first release.
    pub year: Option<i32>,
    /// The year the series ended, if applicable.
    pub end_year: Option<i32>,
    /// The aggregate user rating (0-10).
    pub rating: Option<f32>,
    /// The number of user votes the rating is based on.
    pub votes: Option<u64>,
    /// A short plot description.
    pub plot: Option<String>,
    /// The genres of the title.
    pub genres: Vec<String>,
    /// The series the title is an episode of, when the title is an episode.
    pub series: Option<SeriesInfo>,
}

impl Title {
    /// Checks if the title is an episode of a series.
    #[must_use]
    pub fn is_episode(&self) -> bool {
        self.title_type
            .as_deref()
            .is_some_and(|text| text.eq_ignore_ascii_case("TV Episode"))
    }
}

/// A title matching a search query.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SearchResult {
    /// The unique title id (e.g. `tt1375666`).
    pub id: String,
    /// The display title.
    pub title: Option<String>,
    /// The year of the first release.
    pub year: Option<i32>,
}

/// Details about a person.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Person {
    /// The unique person id (e.g. `nm0186505`).
    pub id: String,
    /// The display name.
    pub name: Option<String>,
    /// The year of birth.
    pub birth_year: Option<i32>,
    /// The year of death, if applicable.
    pub death_year: Option<i32>,
    /// A short biography.
    pub bio: Option<String>,
    /// The titles the person is known for.
    pub known_for: Vec<Title>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episode_marker_is_padded_and_optional() {
        let series = SeriesInfo {
            id: "tt0903747".to_string(),
            title: Some("Breaking Bad".to_string()),
            season: Some("1".to_string()),
            number: Some("7".to_string()),
        };

        assert_eq!(series.episode_marker().as_deref(), Some("S01E07"));

        let series = SeriesInfo::default();

        assert_eq!(series.episode_marker(), None);
    }
}
