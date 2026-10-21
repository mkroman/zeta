//! Fuzzy matching of coin names against the cached coin list.

use strsim::jaro_winkler;

use coinmarketcap::model::Coin;

use super::CoinCache;

impl CoinCache {
    /// Finds the best fuzzy name match for `query`, if any.
    ///
    /// A candidate matches when its name contains the query's characters in order, with at most
    /// `max_name_typos` of them left unmatched. Names containing the query verbatim are preferred
    /// (exact match first, then prefix, then substring); the rest are scored by Jaro-Winkler
    /// similarity, scaled below verbatim matches. The highest score wins, preferring the shorter
    /// name on ties (e.g. `Bitcoin` over `Bitcoin Cash` for the query `bitcoin`).
    pub(super) fn find_by_name(&self, query: &str, max_name_typos: u16) -> Option<&Coin> {
        let needle = query.trim().to_lowercase();

        if needle.is_empty() {
            return None;
        }

        let max_typos = usize::from(max_name_typos);
        let mut lcs = Lcs::default();

        self.by_name
            .iter()
            .filter_map(|(name, symbol)| {
                score_name(&needle, name, max_typos, &mut lcs).map(|score| (score, name, symbol))
            })
            .max_by(|(a_score, a_name, a_symbol), (b_score, b_name, b_symbol)| {
                a_score
                    .total_cmp(b_score)
                    .then_with(|| b_name.len().cmp(&a_name.len()))
                    .then_with(|| a_symbol.cmp(b_symbol))
            })
            .and_then(|(_, _, symbol)| self.by_symbol.get(symbol))
    }
}

/// Scores how well `name` matches `needle` (both lowercased), or `None` if it does not match.
///
/// Verbatim matches rank above fuzzy ones: exact (`1.0`), prefix (`0.95`), then substring
/// (`0.9`). Remaining candidates must match with at most `max_typos` unmatched characters, and
/// are scored by Jaro-Winkler similarity scaled below verbatim matches (Jaro's matching window
/// ignores characters near the end of longer names, so e.g. `gold` must not rely on it to find
/// `PAX Gold`).
fn score_name(needle: &str, name: &str, max_typos: usize, lcs: &mut Lcs) -> Option<f64> {
    if name == needle {
        return Some(1.0);
    }

    if name.starts_with(needle) {
        return Some(0.95);
    }

    if name.contains(needle) {
        return Some(0.9);
    }

    lcs.matches_within(needle, name, max_typos)
        .then(|| jaro_winkler(needle, name) * 0.85)
}

/// Reusable buffers for the longest-common-subsequence check.
///
/// The buffers are reused across the cached coin names, so a fuzzy lookup allocates them once
/// per query instead of once per candidate.
#[derive(Debug, Default)]
struct Lcs {
    /// The query's characters.
    needle: Vec<char>,
    /// The candidate name's characters.
    haystack: Vec<char>,
    /// The previous row of the dynamic program.
    previous: Vec<usize>,
    /// The row of the dynamic program being filled.
    current: Vec<usize>,
}

impl Lcs {
    /// Returns whether `needle` can be matched in `haystack`, in order, with at most `max_typos`
    /// of its characters left unmatched.
    fn matches_within(&mut self, needle: &str, haystack: &str, max_typos: usize) -> bool {
        self.needle.clear();
        self.needle.extend(needle.chars());
        self.haystack.clear();
        self.haystack.extend(haystack.chars());
        self.previous.clear();
        self.previous.resize(self.haystack.len() + 1, 0);
        self.current.clear();
        self.current.resize(self.haystack.len() + 1, 0);

        // The number of unmatched needle characters is the needle length minus the length of
        // the longest common subsequence, computed with the usual row-by-row dynamic program.
        for &needle_char in &self.needle {
            for (index, &haystack_char) in self.haystack.iter().enumerate() {
                let length = if needle_char == haystack_char {
                    self.previous[index] + 1
                } else {
                    self.current[index].max(self.previous[index + 1])
                };

                self.current[index + 1] = length;
            }

            std::mem::swap(&mut self.previous, &mut self.current);
        }

        self.needle.len() - self.previous[self.haystack.len()] <= max_typos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a coin cache for name matching tests.
    fn matching_coins() -> CoinCache {
        CoinCache::from(
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
            .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn fuzzy_matching_allows_missing_and_substituted_characters() {
        let mut lcs = Lcs::default();

        assert!(lcs.matches_within("bitcoin", "bitcoin", 0));
        assert!(lcs.matches_within("bitcon", "bitcoin", 0));
        assert!(lcs.matches_within("bitkoin", "bitcoin", 1));
        assert!(!lcs.matches_within("bitkoin", "bitcoin", 0));
        assert!(!lcs.matches_within("dogecoin", "doge", 0));
    }

    #[test]
    fn prefers_names_containing_the_query() {
        let coins = matching_coins();
        let symbol = |query: &str| {
            coins
                .find_by_name(query, 2)
                .map(|coin| coin.symbol.as_str())
        };

        assert_eq!(symbol("bitcoin"), Some("BTC"));
        assert_eq!(symbol("bitkoin"), Some("BTC"));
        assert_eq!(symbol("gold"), Some("PAXG"));
        assert_eq!(symbol("swap"), Some("UNI"));
        assert_eq!(symbol("graph"), Some("GRT"));
        assert_eq!(symbol("inu"), Some("SHIB"));
    }
}
