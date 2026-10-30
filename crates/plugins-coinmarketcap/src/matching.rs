//! Fuzzy name matching for the cached cryptocurrencies.
//!
//! Names containing the query verbatim are preferred; the rest are scored by Jaro-Winkler
//! similarity, after requiring the query's characters to appear in order with few enough
//! unmatched characters. Symbols are resolved exactly, before this comes into play.
//!
//! The coin cache lower-cases every name once when it is built, so a lookup allocates only the
//! lower-cased query and whatever `jaro_winkler` needs. The length prefilter and the dynamic
//! program run on borrowed data.

use coinmarketcap::model::Coin;
use strsim::jaro_winkler;

/// A coin name prepared for fuzzy matching.
///
/// Lower-casing and counting happen once when the coin cache is built, so scoring a candidate
/// does not allocate.
#[derive(Debug)]
pub struct NamedCoin {
    /// The coin's name, lower-cased for comparison with the lower-cased query.
    name: String,
    /// The number of characters in `name`, used for the length prefilter.
    char_count: usize,
    /// The byte length of the coin's original name, used to prefer shorter names on ties.
    original_len: usize,
    /// The ticker symbol, which keys the coin in the cache.
    symbol: String,
}

impl NamedCoin {
    /// Prepares `coin` for fuzzy matching.
    pub fn new(coin: &Coin) -> Self {
        let name = coin.name.to_lowercase();

        Self {
            char_count: name.chars().count(),
            original_len: coin.name.len(),
            name,
            symbol: coin.symbol.clone(),
        }
    }
}

/// Finds the symbol of the best fuzzy name match for `query` among `coins`, if any.
///
/// A candidate matches when its name contains the query's characters in order, with at most
/// `max_name_typos` of them left unmatched. Names containing the query verbatim are preferred
/// (exact match first, then prefix, then substring); the rest are scored by Jaro-Winkler
/// similarity, scaled below verbatim matches. The highest score wins, preferring the shorter
/// name on ties (e.g. `Bitcoin` over `Bitcoin Cash` for the query `bitcoin`).
pub fn find_by_name<'a>(
    coins: &'a [NamedCoin],
    query: &str,
    max_name_typos: u16,
) -> Option<&'a str> {
    let needle = query.trim().to_lowercase();

    if needle.is_empty() {
        return None;
    }

    let needle_char_count = needle.chars().count();
    let max_typos = usize::from(max_name_typos);

    coins
        .iter()
        .filter_map(|coin| {
            score_name(&needle, needle_char_count, coin, max_typos).map(|score| (score, coin))
        })
        .max_by(|(a_score, a_coin), (b_score, b_coin)| {
            a_score
                .total_cmp(b_score)
                .then_with(|| b_coin.original_len.cmp(&a_coin.original_len))
                .then_with(|| a_coin.symbol.cmp(&b_coin.symbol))
        })
        .map(|(_, coin)| coin.symbol.as_str())
}

/// Scores how well `coin`'s name matches `needle` (both lower-cased), or `None` if it does not
/// match.
///
/// Verbatim matches rank above fuzzy ones: exact (`1.0`), prefix (`0.95`), then substring
/// (`0.9`). Remaining candidates must match with at most `max_typos` unmatched characters, and
/// are scored by Jaro-Winkler similarity scaled below verbatim matches (Jaro's matching window
/// ignores characters near the end of longer names, so e.g. `gold` must not rely on it to find
/// `PAX Gold`).
fn score_name(
    needle: &str,
    needle_char_count: usize,
    coin: &NamedCoin,
    max_typos: usize,
) -> Option<f64> {
    let name = coin.name.as_str();

    if name == needle {
        return Some(1.0);
    }

    if name.starts_with(needle) {
        return Some(0.95);
    }

    if name.contains(needle) {
        return Some(0.9);
    }

    // A common subsequence is at most as long as the shorter text, so a query with more than
    // `max_typos` characters beyond the name's length cannot match. This rejects short names
    // without running the dynamic program.
    if needle_char_count.saturating_sub(coin.char_count) > max_typos {
        return None;
    }

    matches_within(needle, name, max_typos).then(|| jaro_winkler(needle, name) * 0.85)
}

/// The number of dynamic-programming columns that fit in the stack rows.
const ROW_WIDTH: usize = 128;

/// Returns whether `needle` can be matched in `haystack`, in order, with at most `max_typos`
/// of its characters left unmatched.
///
/// The rows of the longest-common-subsequence table live on the stack unless the haystack is
/// longer than [`ROW_WIDTH`] bytes; no allocation is made on the common path.
///
/// The program compares UTF-8 bytes: equal bytes imply equal characters, so a matching name is
/// never rejected, but two different non-ASCII characters can share a byte and be counted as
/// one match. The verbatim, prefix and substring checks in [`score_name`] are exact, so this
/// only relaxes the tolerance filter before Jaro-Winkler.
fn matches_within(needle: &str, haystack: &str, max_typos: usize) -> bool {
    let needle = needle.as_bytes();
    let haystack = haystack.as_bytes();

    // A row counts matched bytes, which the haystack's length bounds. Names this long do not
    // exist; rejecting them keeps the counts inside `u16`.
    if haystack.len() > usize::from(u16::MAX) {
        return false;
    }

    let mut previous_stack = [0u16; ROW_WIDTH];
    let mut current_stack = [0u16; ROW_WIDTH];
    let mut previous_heap = Vec::new();
    let mut current_heap = Vec::new();

    let (mut previous, mut current): (&mut [u16], &mut [u16]) = if haystack.len() < ROW_WIDTH {
        (
            &mut previous_stack[..=haystack.len()],
            &mut current_stack[..=haystack.len()],
        )
    } else {
        previous_heap.resize(haystack.len() + 1, 0);
        current_heap.resize(haystack.len() + 1, 0);
        (&mut previous_heap, &mut current_heap)
    };

    // The number of unmatched needle bytes is the needle length minus the longest common
    // subsequence, computed with the usual row-by-row dynamic program.
    for &needle_byte in needle {
        for (index, &haystack_byte) in haystack.iter().enumerate() {
            current[index + 1] = if needle_byte == haystack_byte {
                previous[index] + 1
            } else {
                current[index].max(previous[index + 1])
            };
        }

        std::mem::swap(&mut previous, &mut current);
    }

    let matched = usize::from(previous[haystack.len()]);

    needle.len() - matched <= max_typos
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

    #[test]
    fn fuzzy_matching_uses_heap_rows_for_long_names() {
        let haystack = "a".repeat(2 * ROW_WIDTH);

        assert!(matches_within("a", &haystack, 0));
        assert!(!matches_within("b", &haystack, 0));
    }

    #[test]
    fn verbatim_matching_is_exact_for_non_ascii_names() {
        let coins = vec![NamedCoin::new(&Coin {
            id: 1,
            name: "Æternity".to_string(),
            symbol: "AE".to_string(),
        })];

        assert_eq!(find_by_name(&coins, "æternity", 0), Some("AE"));
    }

    /// Builds coin names for name matching tests.
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
        .map(|(id, name, symbol)| {
            NamedCoin::new(&Coin {
                id,
                name: name.to_string(),
                symbol: symbol.to_string(),
            })
        })
        .collect()
    }

    #[test]
    fn prefers_names_containing_the_query() {
        let coins = matching_coins();
        let symbol = |query: &str| find_by_name(&coins, query, 2);

        assert_eq!(symbol("bitcoin"), Some("BTC"));
        assert_eq!(symbol("bitkoin"), Some("BTC"));
        assert_eq!(symbol("gold"), Some("PAXG"));
        assert_eq!(symbol("swap"), Some("UNI"));
        assert_eq!(symbol("graph"), Some("GRT"));
        assert_eq!(symbol("inu"), Some("SHIB"));
    }

    /// Times fuzzy matching over a synthetic 5,000-coin corpus.
    ///
    /// ```sh
    /// cargo test -p plugins-coinmarketcap --lib --release -- --ignored --nocapture fuzzy_matching_benchmark
    /// ```
    #[test]
    #[ignore = "benchmark"]
    fn fuzzy_matching_benchmark() {
        use std::collections::HashMap;
        use std::time::Instant;

        const WORDS: [&str; 12] = [
            "Bitcoin",
            "Ethereum",
            "Dogecoin",
            "Solana",
            "Cardano",
            "Polkadot",
            "Ripple",
            "Litecoin",
            "Chainlink",
            "Stellar",
            "Monero",
            "Tezos",
        ];

        let coins: Vec<Coin> = (0..5_000)
            .map(|index| Coin {
                id: u64::try_from(index).expect("index fits in u64") + 1,
                name: format!("{} {}", WORDS[index % WORDS.len()], index % 97),
                symbol: format!("C{index}"),
            })
            .collect();
        let names: Vec<NamedCoin> = coins.iter().map(NamedCoin::new).collect();
        let by_symbol: HashMap<&str, &Coin> = coins
            .iter()
            .map(|coin| (coin.symbol.as_str(), coin))
            .collect();

        let rounds = 200u32;
        // Warm the allocator, the symbol map and the query's lowercase path before timing.
        std::hint::black_box(find_by_name(&names, "bitkoin", 2));
        let start = Instant::now();
        let mut sink = 0usize;
        for _ in 0..rounds {
            // Resolve and clone like `CoinCache::find_by_name` plus its caller, so the
            // benchmark covers the same work before and after the precomputation.
            let found = find_by_name(&names, std::hint::black_box("bitkoin"), 2)
                .and_then(|symbol| by_symbol.get(symbol))
                .copied()
                .cloned();
            sink +=
                std::hint::black_box(found).map_or(0, |coin| usize::try_from(coin.id).unwrap_or(0));
        }
        eprintln!(
            "fuzzy matching: {:?}/lookup (sink {sink})",
            start.elapsed() / rounds
        );
    }
}
