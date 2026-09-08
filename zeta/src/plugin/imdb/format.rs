//! Formats IMDb resources as IRC messages.
//!
//! The scaffolding (labels, parentheses, separators) is colored cyan and the actual values have
//! their color reset, with each value re-entering the cyan mode for the scaffolding that follows.

use std::fmt::Write;

use num_format::{Locale, ToFormattedString};

use super::model::{Person, Title};
use crate::utils::Truncatable;

/// The prefix all IMDb replies start with: a cyan `>` marker, a bold `IMDb` label and a cyan
/// colon.
pub(super) const PREFIX: &str = "\x0310>\x0f\x02 IMDb\x02\x0310:\x0f";

/// Formats the details of `title` as a single IRC message.
#[must_use]
pub fn format_title(title: &Title) -> String {
    let mut message = PREFIX.to_string();

    // Episodes are prefixed with their series and season/episode marker
    // (e.g. `Breaking Bad S01E01: Pilot`).
    if let Some(series) = title.series.as_ref().filter(|_| title.is_episode()) {
        if let Some(series_title) = &series.title {
            let _ = write!(message, " {series_title}\x0310");
        }

        if let Some(marker) = series.episode_marker() {
            let _ = write!(message, " {marker}:\x0f");
        }
    }

    let name = title
        .title
        .as_deref()
        .or(title.original_title.as_deref())
        .unwrap_or(&title.id);
    let _ = write!(message, " {name}\x0310");
    write_years(&mut message, title.year, title.end_year);

    if let Some(plot) = &title.plot {
        let _ = write!(message, " Plot:\x0f {plot}\x0310");
    }

    if let Some(rating) = title.rating {
        let _ = write!(message, " Rating:\x0f {rating:.1}\x0310/\x0f10\x0310");
        let _ = write!(
            message,
            " (\x0f{}\x0310 votes)",
            title
                .votes
                .unwrap_or_default()
                .to_formatted_string(&Locale::en)
        );
    }

    if !title.genres.is_empty() {
        let _ = write!(message, " Genres:");

        for (index, genre) in title.genres.iter().enumerate() {
            if index > 0 {
                let _ = write!(message, ",");
            }

            let _ = write!(message, "\x0f {genre}\x0310");
        }
    }

    let _ = write!(message, " URL:\x0f https://imdb.com/title/{}", title.id);

    message
}

/// Formats the details of `person` as a single IRC message.
#[must_use]
pub fn format_person(person: &Person) -> String {
    let mut message = PREFIX.to_string();
    let name = person.name.as_deref().unwrap_or(&person.id);
    let _ = write!(message, " {name}\x0310");
    write_life_years(&mut message, person.birth_year, person.death_year);

    if !person.known_for.is_empty() {
        let _ = write!(message, " Known for:");

        for (index, title) in person.known_for.iter().enumerate() {
            if index > 0 {
                let _ = write!(message, ",");
            }

            let name = title.title.as_deref().unwrap_or(&title.id);
            let _ = write!(message, "\x0f {name}\x0310");
            write_years(&mut message, title.year, title.end_year);
        }
    }

    if let Some(bio) = &person.bio {
        let _ = write!(
            message,
            " Bio:\x0f {}\x0310",
            bio.truncate_with_suffix(250, "…")
        );
    }

    let _ = write!(message, " URL:\x0f https://imdb.com/name/{}", person.id);

    message
}

/// Writes the release years of a title as `(2008)` or `(2005–2013)`.
fn write_years(message: &mut String, year: Option<i32>, end_year: Option<i32>) {
    let _ = match (year, end_year) {
        (Some(year), Some(end_year)) if end_year != year => {
            write!(message, " (\x0f{year}\x0310–\x0f{end_year}\x0310)")
        }
        (Some(year), _) => write!(message, " (\x0f{year}\x0310)"),
        (None, Some(end_year)) => write!(message, " (?–\x0f{end_year}\x0310)"),
        (None, None) => Ok(()),
    };
}

/// Writes the lifespan of a person as `(b. 1956)`, `(1925–2012)` or `(d. 2012)`.
fn write_life_years(message: &mut String, birth_year: Option<i32>, death_year: Option<i32>) {
    let _ = match (birth_year, death_year) {
        (Some(birth), Some(death)) => write!(message, " (\x0f{birth}\x0310–\x0f{death}\x0310)"),
        (Some(birth), None) => write!(message, " (\x0fb. {birth}\x0310)"),
        (None, Some(death)) => write!(message, " (\x0fd. {death}\x0310)"),
        (None, None) => Ok(()),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::imdb::model::SeriesInfo;

    #[test]
    fn format_title_matches_expected_layout() {
        let title = Title {
            id: "tt14663588".to_string(),
            title: Some("Peggle Nights".to_string()),
            original_title: Some("Peggle Nights".to_string()),
            title_type: Some("Video Game".to_string()),
            year: Some(2008),
            end_year: None,
            rating: Some(7.6),
            votes: Some(51),
            plot: Some("Downloadable follow-up to the original \"Peggle (2007)\".".to_string()),
            genres: vec![
                "Action".to_string(),
                "Adventure".to_string(),
                "Family".to_string(),
            ],
            series: None,
        };

        assert_eq!(
            format_title(&title),
            "\x0310>\x0f\x02 IMDb\x02\x0310:\x0f Peggle Nights\x0310 (\x0f2008\x0310) Plot:\x0f Downloadable follow-up to the original \"Peggle (2007)\".\x0310 Rating:\x0f 7.6\x0310/\x0f10\x0310 (\x0f51\x0310 votes) Genres:\x0f Action\x0310,\x0f Adventure\x0310,\x0f Family\x0310 URL:\x0f https://imdb.com/title/tt14663588"
        );
    }

    #[test]
    fn format_episode_is_prefixed_with_series_and_marker() {
        let title = Title {
            id: "tt0959621".to_string(),
            title: Some("Pilot".to_string()),
            title_type: Some("TV Episode".to_string()),
            year: Some(2008),
            rating: Some(9.1),
            votes: Some(29_553),
            plot: Some("When an unassuming high school chemistry teacher learns he has cancer, he turns to a life of crime.".to_string()),
            series: Some(SeriesInfo {
                id: "tt0903747".to_string(),
                title: Some("Breaking Bad".to_string()),
                season: Some("1".to_string()),
                number: Some("1".to_string()),
            }),
            ..Title::default()
        };

        assert_eq!(
            format_title(&title),
            "\x0310>\x0f\x02 IMDb\x02\x0310:\x0f Breaking Bad\x0310 S01E01:\x0f Pilot\x0310 (\x0f2008\x0310) Plot:\x0f When an unassuming high school chemistry teacher learns he has cancer, he turns to a life of crime.\x0310 Rating:\x0f 9.1\x0310/\x0f10\x0310 (\x0f29,553\x0310 votes) URL:\x0f https://imdb.com/title/tt0959621"
        );
    }

    #[test]
    fn format_ignores_series_info_for_non_episodes() {
        let title = Title {
            id: "tt1375666".to_string(),
            title: Some("Inception".to_string()),
            title_type: Some("Movie".to_string()),
            year: Some(2010),
            series: Some(SeriesInfo {
                id: "tt0903747".to_string(),
                title: Some("Breaking Bad".to_string()),
                season: Some("1".to_string()),
                number: Some("1".to_string()),
            }),
            ..Title::default()
        };

        let formatted = format_title(&title);

        assert!(formatted.contains("\x0f Inception\x0310 (\x0f2010\x0310)"));
        assert!(!formatted.contains("Breaking Bad"));
    }

    #[test]
    fn format_title_omits_missing_fields() {
        let title = Title {
            id: "tt0000001".to_string(),
            ..Title::default()
        };

        assert_eq!(
            format_title(&title),
            "\x0310>\x0f\x02 IMDb\x02\x0310:\x0f tt0000001\x0310 URL:\x0f https://imdb.com/title/tt0000001"
        );
    }

    #[test]
    fn write_years_includes_end_year_for_series() {
        let mut message = String::new();
        write_years(&mut message, Some(2008), None);
        assert_eq!(message, " (\x0f2008\x0310)");

        let mut message = String::new();
        write_years(&mut message, Some(2005), Some(2013));
        assert_eq!(message, " (\x0f2005\x0310–\x0f2013\x0310)");

        let mut message = String::new();
        write_years(&mut message, None, None);
        assert_eq!(message, "");
    }

    #[test]
    fn format_person_matches_expected_layout() {
        let person = Person {
            id: "nm0186505".to_string(),
            name: Some("Bryan Cranston".to_string()),
            birth_year: Some(1956),
            death_year: None,
            bio: Some("Bryan Lee Cranston was born on March 7, 1956.".to_string()),
            known_for: vec![Title {
                id: "tt0903747".to_string(),
                title: Some("Breaking Bad".to_string()),
                year: Some(2008),
                end_year: Some(2013),
                ..Title::default()
            }],
        };

        assert_eq!(
            format_person(&person),
            "\x0310>\x0f\x02 IMDb\x02\x0310:\x0f Bryan Cranston\x0310 (\x0fb. 1956\x0310) Known for:\x0f Breaking Bad\x0310 (\x0f2008\x0310–\x0f2013\x0310) Bio:\x0f Bryan Lee Cranston was born on March 7, 1956.\x0310 URL:\x0f https://imdb.com/name/nm0186505"
        );
    }

    #[test]
    fn format_person_truncates_bio() {
        let person = Person {
            id: "nm0000138".to_string(),
            name: Some("Leo".to_string()),
            bio: Some("x".repeat(300)),
            ..Person::default()
        };

        let formatted = format_person(&person);

        assert!(formatted.contains("xxx…\x0310 URL:\x0f"));
    }

    #[test]
    fn write_life_years_variants() {
        let mut message = String::new();
        write_life_years(&mut message, Some(1956), None);
        assert_eq!(message, " (\x0fb. 1956\x0310)");

        let mut message = String::new();
        write_life_years(&mut message, Some(1925), Some(2012));
        assert_eq!(message, " (\x0f1925\x0310–\x0f2012\x0310)");

        let mut message = String::new();
        write_life_years(&mut message, None, Some(2012));
        assert_eq!(message, " (\x0fd. 2012\x0310)");

        let mut message = String::new();
        write_life_years(&mut message, None, None);
        assert_eq!(message, "");
    }
}
