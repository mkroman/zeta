//! Tests decoding recorded GraphQL responses.
//!
//! The fixtures are recorded API responses, so a renamed or reordered field in a query fails here
//! without a network call.

use super::*;

/// Reads a recorded GraphQL response fixture.
fn fixture(name: &str) -> String {
    zeta_test_support::fixture!(format!("imdb/{name}.json"))
}

#[test]
fn decodes_recorded_search_response() {
    let text = fixture("search_peggle_nights");

    let response: GraphQlResponse<SearchData> = http::json::from_str(&text).unwrap();
    let data = response.data.unwrap();
    let results: Vec<SearchResult> = data
        .main_search
        .unwrap()
        .edges
        .into_iter()
        .filter_map(|edge| edge.node.entity)
        .map(SearchResult::from)
        .collect();

    assert_ne!(results, Vec::new());
    assert_eq!(results[0].id, "tt14663588");
    assert_eq!(results[0].title.as_deref(), Some("Peggle Nights"));
    assert_eq!(results[0].year, Some(2008));
}

#[test]
fn decodes_recorded_title_response() {
    let text = fixture("title_tt14663588");

    let response: GraphQlResponse<TitleData> = http::json::from_str(&text).unwrap();
    let title = Title::from(response.data.unwrap().title.unwrap());

    assert_eq!(title.id, "tt14663588");
    assert_eq!(title.title.as_deref(), Some("Peggle Nights"));
    assert_eq!(title.original_title.as_deref(), Some("Peggle Nights"));
    assert_eq!(title.title_type.as_deref(), Some("Video Game"));
    assert_eq!(title.year, Some(2008));
    assert_eq!(title.end_year, None);
    assert_eq!(title.rating, Some(7.6));
    assert_eq!(title.votes, Some(53));
    assert!(title.plot.as_deref().unwrap().starts_with("Downloadable"));
    assert_eq!(title.genres, ["Action", "Adventure", "Family"]);
    assert!(title.series.is_none());
    assert!(!title.is_episode());
}

#[test]
fn decodes_missing_titles_as_not_found() {
    // Non-existent titles are returned as an empty node (all fields null), not `null`.
    let empty_node = fixture("title_not_found");
    let null_node = r#"{"data":{"title":null}}"#;

    for (case, text) in [
        ("empty node", empty_node),
        ("null node", null_node.to_string()),
    ] {
        let response: GraphQlResponse<TitleData> = http::json::from_str(&text).unwrap();

        assert!(
            matches!(
                title_from_node(response.data.unwrap().title),
                Err(Error::NotFound)
            ),
            "{case}"
        );
    }
}

#[test]
fn decodes_recorded_episode_response() {
    let text = fixture("title_tt0959621");

    let response: GraphQlResponse<TitleData> = http::json::from_str(&text).unwrap();
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
fn decodes_recorded_name_response() {
    let text = fixture("name_nm0186505");

    let response: GraphQlResponse<NameData> = http::json::from_str(&text).unwrap();
    let person = Person::from(response.data.unwrap().name.unwrap());

    assert_eq!(person.id, "nm0186505");
    assert_eq!(person.name.as_deref(), Some("Bryan Cranston"));
    assert_eq!(person.birth_year, Some(1956));
    assert_eq!(person.death_year, None);
    assert!(
        person
            .bio
            .as_deref()
            .unwrap()
            .starts_with("Bryan Lee Cranston was born on March 7, 1956")
    );
    assert_ne!(person.known_for, Vec::new());
    assert_eq!(person.known_for[0].title.as_deref(), Some("Breaking Bad"));
}
