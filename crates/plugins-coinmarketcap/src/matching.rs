//! Fuzzy name matching for the cached cryptocurrencies.
//!
//! Names containing the query verbatim are preferred; the rest are scored by Jaro-Winkler
//! similarity, after requiring the query's characters to appear in order with few enough
//! unmatched characters. Symbols are resolved exactly, before this comes into play.
//!
//! The coin cache lower-cases every name once when it is built, so a lookup allocates only the
//! lower-cased query and whatever `jaro_winkler` needs. The length prefilter and the dynamic
//! program run on borrowed data, and the program's scratch rows are reused across candidates.

use coinmarketcap::model::Coin;
use strsim::jaro_winkler;

/// A coin name prepared for fuzzy matching.
///
/// Lower-casing and counting happen once when the coin cache is built, so scoring a candidate
/// does not allocate.
#[derive(Debug)]
pub struct NamedCoin {
    /// The CoinMarketCap id of the coin, returned for the best match.
    id: u64,
    /// The coin's name, lower-cased for comparison with the lower-cased query.
    name: String,
    /// The number of characters in `name`, used for the length prefilter.
    char_count: usize,
    /// The byte length of the coin's original name, used to prefer shorter names on ties.
    original_len: usize,
    /// The ticker symbol, used to break ties between equally long names.
    symbol: String,
}

impl NamedCoin {
    /// Prepares `coin` for fuzzy matching.
    pub fn new(coin: &Coin) -> Self {
        let name = coin.name.to_lowercase();

        Self {
            id: coin.id,
            char_count: name.chars().count(),
            original_len: coin.name.len(),
            name,
            symbol: coin.symbol.clone(),
        }
    }
}

/// Finds the id of the best fuzzy name match for `query` among `coins`, if any.
///
/// A candidate matches when its name contains the query's characters in order, with at most
/// `max_name_typos` of them left unmatched. Names containing the query verbatim are preferred
/// (exact match first, then prefix, then substring); the rest are scored by Jaro-Winkler
/// similarity, scaled below verbatim matches. The highest score wins, preferring the shorter
/// name on ties (e.g. `Bitcoin` over `Bitcoin Cash` for the query `bitcoin`).
pub fn find_by_name(coins: &[NamedCoin], query: &str, max_name_typos: u16) -> Option<u64> {
    let needle = query.trim().to_lowercase();

    if needle.is_empty() {
        return None;
    }

    let needle_char_count = needle.chars().count();
    let max_typos = usize::from(max_name_typos);
    let mut rows = DpRows::new();

    coins
        .iter()
        .filter_map(|coin| {
            score_name(&needle, needle_char_count, coin, max_typos, &mut rows)
                .map(|score| (score, coin))
        })
        .max_by(|(a_score, a_coin), (b_score, b_coin)| {
            a_score
                .total_cmp(b_score)
                .then_with(|| b_coin.original_len.cmp(&a_coin.original_len))
                .then_with(|| a_coin.symbol.cmp(&b_coin.symbol))
        })
        .map(|(_, coin)| coin.id)
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
    rows: &mut DpRows,
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

    matches_within(needle, name, max_typos, rows).then(|| jaro_winkler(needle, name) * 0.85)
}

/// The number of dynamic-programming columns that fit in the stack rows.
const ROW_WIDTH: usize = 128;

/// The two rows of the longest-common-subsequence table.
///
/// One set is reused across all the candidate names of a lookup, so scanning the coin list
/// allocates its heap rows at most once. The rows live on the stack unless the name is longer
/// than [`ROW_WIDTH`] bytes.
struct DpRows {
    /// The row above the current one, for names that fit on the stack.
    previous: [u16; ROW_WIDTH],
    /// The row being written, for names that fit on the stack.
    current: [u16; ROW_WIDTH],
    /// The row above the current one, for longer names.
    previous_long: Vec<u16>,
    /// The row being written, for longer names.
    current_long: Vec<u16>,
}

impl DpRows {
    /// Creates empty scratch rows.
    const fn new() -> Self {
        Self {
            previous: [0; ROW_WIDTH],
            current: [0; ROW_WIDTH],
            previous_long: Vec::new(),
            current_long: Vec::new(),
        }
    }

    /// Returns zeroed rows for a haystack of `haystack_len` bytes.
    ///
    /// Only the prefix the program uses is reset: values left behind by a longer candidate
    /// would otherwise be read as matches.
    fn for_haystack(&mut self, haystack_len: usize) -> (&mut [u16], &mut [u16]) {
        let (previous, current): (&mut [u16], &mut [u16]) = if haystack_len < ROW_WIDTH {
            (
                &mut self.previous[..=haystack_len],
                &mut self.current[..=haystack_len],
            )
        } else {
            self.previous_long.resize(haystack_len + 1, 0);
            self.current_long.resize(haystack_len + 1, 0);

            (&mut self.previous_long, &mut self.current_long)
        };

        previous.fill(0);
        current.fill(0);

        (previous, current)
    }
}

/// Returns whether `needle` can be matched in `haystack`, in order, with at most `max_typos`
/// of its characters left unmatched.
///
/// The rows of the longest-common-subsequence table live on the stack unless the haystack is
/// longer than [`ROW_WIDTH`] bytes; no allocation is made for short names, and the long-name
/// rows are reused across the candidates of a lookup through `rows`.
///
/// The program compares UTF-8 bytes: equal bytes imply equal characters, so a matching name is
/// never rejected, but two different non-ASCII characters can share a byte and be counted as
/// one match. The verbatim, prefix and substring checks in [`score_name`] are exact, so this
/// only relaxes the tolerance filter before Jaro-Winkler.
fn matches_within(needle: &str, haystack: &str, max_typos: usize, rows: &mut DpRows) -> bool {
    let needle = needle.as_bytes();
    let haystack = haystack.as_bytes();

    // A row counts matched bytes, which the haystack's length bounds. Names this long do not
    // exist; rejecting them keeps the counts inside `u16`.
    if haystack.len() > usize::from(u16::MAX) {
        return false;
    }

    let (mut previous, mut current) = rows.for_haystack(haystack.len());

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

    /// Runs `matches_within` with fresh scratch, as a lookup's first candidate does.
    fn within(needle: &str, haystack: &str, max_typos: usize) -> bool {
        matches_within(needle, haystack, max_typos, &mut DpRows::new())
    }

    #[test]
    fn fuzzy_matching_allows_missing_and_substituted_characters() {
        assert!(within("bitcoin", "bitcoin", 0));
        assert!(within("bitcon", "bitcoin", 0));
        assert!(within("bitkoin", "bitcoin", 1));
        assert!(!within("bitkoin", "bitcoin", 0));
        assert!(!within("dogecoin", "doge", 0));
    }

    #[test]
    fn fuzzy_matching_uses_heap_rows_for_long_names() {
        let haystack = "a".repeat(2 * ROW_WIDTH);

        assert!(within("a", &haystack, 0));
        assert!(!within("b", &haystack, 0));
    }

    #[test]
    fn reused_rows_match_fresh_rows() {
        // A reused lookup must reset the rows a shorter or longer candidate left behind.
        let haystacks = [
            "bitcoin".to_string(),
            "bitcoin cash".to_string(),
            "a".repeat(2 * ROW_WIDTH),
            "b".repeat(3 * ROW_WIDTH / 2),
            "bitcoin".repeat(30),
            "doge".to_string(),
        ];
        let needles = ["bitcoin", "bitkoin", "doge", "b", "zz"];

        let mut rows = DpRows::new();
        for haystack in &haystacks {
            for needle in needles {
                assert_eq!(
                    matches_within(needle, haystack, 2, &mut rows),
                    within(needle, haystack, 2),
                    "needle {needle:?} against {haystack:?}"
                );
            }
        }
    }

    #[test]
    fn verbatim_matching_is_exact_for_non_ascii_names() {
        let coins = vec![NamedCoin::new(&Coin {
            id: 1,
            name: "Æternity".to_string(),
            symbol: "AE".to_string(),
        })];

        assert_eq!(find_by_name(&coins, "æternity", 0), Some(1));
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
        let id = |query: &str| find_by_name(&coins, query, 2);

        assert_eq!(id("bitcoin"), Some(1));
        assert_eq!(id("bitkoin"), Some(1));
        assert_eq!(id("gold"), Some(3));
        assert_eq!(id("swap"), Some(5));
        assert_eq!(id("graph"), Some(7));
        assert_eq!(id("inu"), Some(8));
    }

    /// Times fuzzy matching over a synthetic 5,000-coin corpus.
    ///
    /// ```sh
    /// cargo test -p plugins-coinmarketcap --lib --release -- --ignored --nocapture fuzzy_matching_benchmark
    /// ```
    #[test]
    #[ignore = "benchmark"]
    fn fuzzy_matching_benchmark() {
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

        let rounds = 200u32;
        // Warm the allocator and the query's lowercase path before timing.
        std::hint::black_box(find_by_name(&names, "bitkoin", 2));
        let start = Instant::now();
        let mut sink = 0usize;
        for _ in 0..rounds {
            // Resolve like `resolve_coin`, so the benchmark covers the same work.
            let found = find_by_name(&names, std::hint::black_box("bitkoin"), 2);
            sink += std::hint::black_box(found).map_or(0, |id| usize::try_from(id).unwrap_or(0));
        }
        eprintln!(
            "fuzzy matching: {:?}/lookup (sink {sink})",
            start.elapsed() / rounds
        );
    }
}
