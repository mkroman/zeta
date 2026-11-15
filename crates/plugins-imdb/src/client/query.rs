//! The GraphQL documents the client sends.
//!
//! Each public operation has one query, named after it in the request's `operationName`. The
//! field selection mirrors the response nodes in `model`, so a field added here needs a matching
//! node field there.

/// Query fetching the details of a single title.
pub(super) const GET_TITLE_QUERY: &str = r"
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
pub(super) const GET_NAME_QUERY: &str = r"
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
pub(super) const SEARCH_QUERY: &str = r"
query Search($searchTerm: String!, $first: Int!, $includeAdult: Boolean!) {
  mainSearch(first: $first, options: {searchTerm: $searchTerm, type: TITLE, includeAdult: $includeAdult}) {
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
