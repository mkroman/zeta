//! Fuzzy name matching for the cached cryptocurrencies.
//!
//! Names containing the query verbatim are preferred; the rest are scored by Jaro-Winkler
//! similarity, after requiring the query's characters to appear in order with few enough
//! unmatched characters. Symbols are resolved exactly, before this comes into play.

use std::collections::HashMap;

use coinmarketcap::model::Coin;
use strsim::jaro_winkler;

/// Finds the best fuzzy name match for `query` among `coins`, if any.
///
/// A candidate matches when its name contains the query's characters in order, with at most
/// `max_name_typos` of them left unmatched. Names containing the query verbatim are preferred
/// (exact match first, then prefix, then substring); the rest are scored by Jaro-Winkler
/// similarity, scaled below verbatim matches. The highest score wins, preferring the shorter
/// name on ties (e.g. `Bitcoin` over `Bitcoin Cash` for the query `bitcoin`).
pub fn find_by_name(
    coins: &HashMap<String, Coin>,
    query: &str,
    max_name_typos: u16,
) -> Option<Coin> {
    let needle = query.trim().to_lowercase();

    if needle.is_empty() {
        return None;
    }

    let max_typos = usize::from(max_name_typos);

    coins
        .values()
        .filter_map(|coin| {
            let name = coin.name.to_lowercase();

            score_name(&needle, &name, max_typos).map(|score| (score, coin))
        })
        .max_by(|(a_score, a_coin), (b_score, b_coin)| {
            a_score
                .total_cmp(b_score)
                .then_with(|| b_coin.name.len().cmp(&a_coin.name.len()))
                .then_with(|| a_coin.symbol.cmp(&b_coin.symbol))
        })
        .map(|(_, coin)| coin.clone())
}

/// Scores how well `name` matches `needle` (both lowercased), or `None` if it does not match.
///
/// Verbatim matches rank above fuzzy ones: exact (`1.0`), prefix (`0.95`), then substring
/// (`0.9`). Remaining candidates must match with at most `max_typos` unmatched characters, and
/// are scored by Jaro-Winkler similarity scaled below verbatim matches (Jaro's matching window
/// ignores characters near the end of longer names, so e.g. `gold` must not rely on it to find
/// `PAX Gold`).
fn score_name(needle: &str, name: &str, max_typos: usize) -> Option<f64> {
    if name == needle {
        return Some(1.0);
    }

    if name.starts_with(needle) {
        return Some(0.95);
    }

    if name.contains(needle) {
        return Some(0.9);
    }

    matches_within(needle, name, max_typos).then(|| jaro_winkler(needle, name) * 0.85)
}

/// Returns whether `needle` can be matched in `haystack`, in order, with at most `max_typos`
/// of its characters left unmatched.
fn matches_within(needle: &str, haystack: &str, max_typos: usize) -> bool {
    let needle: Vec<char> = needle.chars().collect();
    let haystack: Vec<char> = haystack.chars().collect();

    // The number of unmatched needle characters is the needle length minus the length of the
    // longest common subsequence, computed with the usual row-by-row dynamic program.
    let mut previous = vec![0; haystack.len() + 1];
    let mut current = vec![0; haystack.len() + 1];

    for &needle_char in &needle {
        for (index, &haystack_char) in haystack.iter().enumerate() {
            current[index + 1] = if needle_char == haystack_char {
                previous[index] + 1
            } else {
                current[index].max(previous[index + 1])
            };
        }

        std::mem::swap(&mut previous, &mut current);
    }

    needle.len() - previous[haystack.len()] <= max_typos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matching_allows_missing_and_substituted_characters() {
        assert!(matches_within("bitcoin", "bitcoin", 0));
        assert!(matches_within("bitcon", "bitcoin", 0));
        assert!(matches_within("bitkoin", "bitcoin", 1));
        assert!(!matches_within("bitkoin", "bitcoin", 0));
        assert!(!matches_within("dogecoin", "doge", 0));
    }

    /// Builds a coin map keyed by symbol for name matching tests.
    fn matching_coins() -> HashMap<String, Coin> {
        [
            (1, "Bitcoin", "BTC"),
            (2, "Bitcoin Cash", "BCH"),
            (3, "PAX Gold", "PAXG"),
            (4, "Golem", "GLM"),
            (5, "Uniswap", "UNI"),
            (6, "SushiSwap", "SUSHI"),
            (7, "The Graph", "GRT"),
            (8, "Shiba Inu", "SHIB"),
        ]
        .into_iter()
        .map(|(id, name, symbol)| {
            (
                symbol.to_string(),
                Coin {
                    id,
                    name: name.to_string(),
                    symbol: symbol.to_string(),
                },
            )
        })
        .collect()
    }

    #[test]
    fn prefers_names_containing_the_query() {
        let coins = matching_coins();
        let symbol = |query: &str| find_by_name(&coins, query, 2).map(|coin| coin.symbol);

        assert_eq!(symbol("bitcoin").as_deref(), Some("BTC"));
        assert_eq!(symbol("bitkoin").as_deref(), Some("BTC"));
        assert_eq!(symbol("gold").as_deref(), Some("PAXG"));
        assert_eq!(symbol("swap").as_deref(), Some("UNI"));
        assert_eq!(symbol("graph").as_deref(), Some("GRT"));
        assert_eq!(symbol("inu").as_deref(), Some("SHIB"));
    }
}
