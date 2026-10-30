//! Fuzzy name matching for the cached cryptocurrencies.
//!
//! Names containing the query verbatim are preferred; the rest are scored by Jaro-Winkler
//! similarity, after requiring the query's characters to appear in order with few enough
//! unmatched characters. Symbols are resolved exactly, before this comes into play.

use coinmarketcap::model::Coin;
use strsim::jaro_winkler;

/// A coin beside its lowercased name, the form the fuzzy matcher scores against.
pub type NamedCoin = (String, Coin);

/// Finds the coin that best fuzzy-matches `query` by name among `names`, if any.
///
/// A candidate matches when its name contains the query's characters in order, with at most
/// `max_name_typos` of them left unmatched. Names containing the query verbatim are preferred
/// (exact match first, then prefix, then substring); the rest are scored by Jaro-Winkler
/// similarity, scaled below verbatim matches. The highest score wins, preferring the shorter
/// name on ties (e.g. `Bitcoin` over `Bitcoin Cash` for the query `bitcoin`).
///
/// The candidate names arrive lowercased, computed once per cache build rather than once per
/// candidate per query, and the dynamic-programming scratch rows are reused across candidates:
/// a lookup allocates for the needle alone, not once per coin.
pub fn find_by_name<'a>(
    names: &'a [NamedCoin],
    query: &str,
    max_name_typos: u16,
) -> Option<&'a Coin> {
    let needle = query.trim().to_lowercase();

    if needle.is_empty() {
        return None;
    }

    let max_typos = usize::from(max_name_typos);
    let needle_chars: Vec<char> = needle.chars().collect();
    let mut rows = DpRows::default();

    names
        .iter()
        .filter_map(|(name, coin)| {
            score_name(&needle, &needle_chars, name, max_typos, &mut rows)
                .map(|score| (score, coin))
        })
        .max_by(|(a_score, a_coin), (b_score, b_coin)| {
            a_score
                .total_cmp(b_score)
                .then_with(|| b_coin.name.len().cmp(&a_coin.name.len()))
                .then_with(|| a_coin.symbol.cmp(&b_coin.symbol))
        })
        .map(|(_, coin)| coin)
}

/// Scores how well `name` (lowercased) matches `needle` (lowercased), or [`None`] if it does
/// not match.
///
/// Verbatim matches rank above fuzzy ones: exact (`1.0`), prefix (`0.95`), then substring
/// (`0.9`). Remaining candidates must match with at most `max_typos` unmatched characters, and
/// are scored by Jaro-Winkler similarity scaled below verbatim matches (Jaro's matching window
/// ignores characters near the end of longer names, so e.g. `gold` must not rely on it to find
/// `PAX Gold`).
fn score_name(
    needle: &str,
    needle_chars: &[char],
    name: &str,
    max_typos: usize,
    rows: &mut DpRows,
) -> Option<f64> {
    if name == needle {
        return Some(1.0);
    }

    if name.starts_with(needle) {
        return Some(0.95);
    }

    if name.contains(needle) {
        return Some(0.9);
    }

    matches_within(needle_chars, name, max_typos, rows).then(|| jaro_winkler(needle, name) * 0.85)
}

/// The two scratch rows of the longest-common-subsequence dynamic program, reused across
/// candidate names.
#[derive(Default)]
struct DpRows {
    previous: Vec<usize>,
    current: Vec<usize>,
}

/// Returns whether `needle` can be matched in `haystack`, in order, with at most `max_typos`
/// of its characters left unmatched.
///
/// The number of unmatched needle characters is the needle length minus the length of the
/// longest common subsequence, computed with the usual row-by-row dynamic program over
/// `rows`. The rows are only ever grown, and their used prefix is zeroed per candidate, so
/// scanning the whole coin list allocates its scratch once and never per candidate.
fn matches_within(needle: &[char], haystack: &str, max_typos: usize, rows: &mut DpRows) -> bool {
    // A common subsequence is never longer than the shorter side, so a needle this much
    // longer than the haystack already has too many unmatched characters.
    let haystack_len = haystack.chars().count();

    if needle.len() > haystack_len + max_typos {
        return false;
    }

    if rows.previous.len() < haystack_len + 1 {
        rows.previous.resize(haystack_len + 1, 0);
        rows.current.resize(haystack_len + 1, 0);
    }

    // Cells beyond the current haystack length are stale from a longer candidate; zeroing the
    // used prefix resets the program for this one.
    rows.previous[..=haystack_len].fill(0);
    rows.current[..=haystack_len].fill(0);

    let (previous, current) = (&mut rows.previous, &mut rows.current);

    for &needle_char in needle {
        for (index, haystack_char) in haystack.chars().enumerate() {
            current[index + 1] = if needle_char == haystack_char {
                previous[index] + 1
            } else {
                current[index].max(previous[index + 1])
            };
        }

        std::mem::swap(previous, current);
    }

    needle.len() - previous[haystack_len] <= max_typos
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `matches_within` with the scratch the production path reuses.
    fn within(needle: &str, haystack: &str, max_typos: usize) -> bool {
        let needle: Vec<char> = needle.chars().collect();

        matches_within(&needle, haystack, max_typos, &mut DpRows::default())
    }

    #[test]
    fn fuzzy_matching_allows_missing_and_substituted_characters() {
        assert!(within("bitcoin", "bitcoin", 0));
        assert!(within("bitcon", "bitcoin", 0));
        assert!(within("bitkoin", "bitcoin", 1));
        assert!(!within("bitkoin", "bitcoin", 0));
        assert!(!within("dogecoin", "doge", 0));
    }

    /// Builds a lowercased-name list for name matching tests, the way the cache builds it.
    fn matching_coins() -> Vec<NamedCoin> {
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
        .map(|(id, name, symbol)| Coin {
            id,
            name: name.to_string(),
            symbol: symbol.to_string(),
        })
        .map(|coin| (coin.name.to_lowercase(), coin))
        .collect()
    }

    #[test]
    fn prefers_names_containing_the_query() {
        let coins = matching_coins();
        let symbol = |query: &str| find_by_name(&coins, query, 2).map(|coin| coin.symbol.clone());

        assert_eq!(symbol("bitcoin").as_deref(), Some("BTC"));
        assert_eq!(symbol("bitkoin").as_deref(), Some("BTC"));
        assert_eq!(symbol("gold").as_deref(), Some("PAXG"));
        assert_eq!(symbol("swap").as_deref(), Some("UNI"));
        assert_eq!(symbol("graph").as_deref(), Some("GRT"));
        assert_eq!(symbol("inu").as_deref(), Some("SHIB"));
    }

    #[test]
    fn a_needle_longer_than_the_name_can_never_match() {
        // One unmatched needle character is allowed, but two are not: the length pre-filter
        // must agree with the dynamic program on where that line sits.
        assert!(within("doge", "dog", 1));
        assert!(!within("doges", "dog", 1));
        assert!(!within("doge", "dog", 0));
    }
}
