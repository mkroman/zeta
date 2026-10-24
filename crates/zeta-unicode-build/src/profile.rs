//! Training profiles for the Nomina image.
//!
//! A profile says which name-block shifts training measures and how it picks one. The choice
//! is deterministic: candidates are sorted and deduplicated, ties break toward the lower
//! shift, and nothing depends on the order the caller listed the candidates in.
//!
//! The block shift is the number of names in one front-coded block, as a power of two:
//! shift 3 is 8 names per block, shift 8 is 256. Smaller blocks cost more index bytes but
//! decode less per name lookup; larger blocks shrink the image.

use std::fmt;

/// The smallest name-block shift a profile may train with.
pub const MIN_CANDIDATE_SHIFT: u8 = 3;
/// The largest name-block shift a profile may train with.
pub const MAX_CANDIDATE_SHIFT: u8 = 8;
/// The largest shift [`Priority::Balanced`] prefers before falling back to the smallest image.
pub const BALANCED_MAX_SHIFT: u8 = 6;
/// The default image budget, in bytes.
pub const DEFAULT_MAX_TABLE_BYTES: usize = 200_000;
/// The default candidate shifts: 32, 64, 128 and 256 names per block.
pub const DEFAULT_CANDIDATE_SHIFTS: [u8; 4] = [5, 6, 7, 8];

/// How training chooses between candidate name-block shifts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// The smallest image; equal totals tie toward the lower shift.
    Size,
    /// The smallest image among shifts at or below [`BALANCED_MAX_SHIFT`], ties toward the
    /// lower shift. When every candidate is above that limit, the smallest image wins.
    Balanced,
    /// The lowest shift, which decodes the least data per name lookup.
    Latency,
}

impl Priority {
    /// The stable lower-case name used by the command line and the manifest.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Balanced => "balanced",
            Self::Latency => "latency",
        }
    }

    /// Parses [`Self::as_str`] case-insensitively.
    #[must_use]
    pub const fn parse(text: &str) -> Option<Self> {
        if text.eq_ignore_ascii_case("size") {
            Some(Self::Size)
        } else if text.eq_ignore_ascii_case("balanced") {
            Some(Self::Balanced)
        } else if text.eq_ignore_ascii_case("latency") {
            Some(Self::Latency)
        } else {
            None
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The parameters a training run should honour.
///
/// [`Self::name_block_shift`] pins one shift; when it is `None`, training measures
/// [`Self::candidates`] and [`Self::priority`] chooses the winner. A baseline image compares
/// the trained size against an existing image, allowing at most [`Self::max_delta_bytes`]
/// bytes of growth.
#[derive(Clone, Debug)]
pub struct Profile {
    /// The rule used to choose between candidates.
    pub priority: Priority,
    /// A fixed block shift, or `None` to search [`Self::candidates`].
    pub name_block_shift: Option<u8>,
    /// The block shifts to measure when no fixed shift is set.
    pub candidates: Vec<u8>,
    /// The largest image training may produce, in bytes.
    pub max_table_bytes: usize,
    /// An existing image whose total size is the baseline, if any.
    pub baseline: Option<Vec<u8>>,
    /// How many bytes larger than the baseline are tolerated.
    pub max_delta_bytes: usize,
}

impl Profile {
    /// A profile that searches [`DEFAULT_CANDIDATE_SHIFTS`] with `priority` and `budget`.
    #[must_use]
    pub fn new(priority: Priority, max_table_bytes: usize) -> Self {
        Self {
            priority,
            name_block_shift: None,
            candidates: DEFAULT_CANDIDATE_SHIFTS.to_vec(),
            max_table_bytes,
            baseline: None,
            max_delta_bytes: 0,
        }
    }

    /// Pins the block shift, disabling the candidate search.
    #[must_use]
    pub const fn with_block_shift(mut self, shift: u8) -> Self {
        self.name_block_shift = Some(shift);
        self
    }

    /// Replaces the candidate list.
    #[must_use]
    pub fn with_candidates(mut self, shifts: impl IntoIterator<Item = u8>) -> Self {
        self.candidates = shifts.into_iter().collect();
        self
    }

    /// Compares the trained image against `image`, allowing `max_delta_bytes` of growth.
    #[must_use]
    pub fn with_baseline(mut self, image: impl Into<Vec<u8>>, max_delta_bytes: usize) -> Self {
        self.baseline = Some(image.into());
        self.max_delta_bytes = max_delta_bytes;
        self
    }

    /// Sets the tolerated growth over [`Self::baseline`].
    #[must_use]
    pub const fn with_max_delta_bytes(mut self, max_delta_bytes: usize) -> Self {
        self.max_delta_bytes = max_delta_bytes;
        self
    }

    /// The candidates in the order training measures them.
    ///
    /// A fixed [`Self::name_block_shift`] wins over [`Self::candidates`]; otherwise the list
    /// is sorted and deduplicated so two profiles with the same set behave identically.
    #[must_use]
    pub fn resolved_candidates(&self) -> Vec<u8> {
        if let Some(shift) = self.name_block_shift {
            return vec![shift];
        }
        let mut shifts = self.candidates.clone();
        shifts.sort_unstable();
        shifts.dedup();
        shifts
    }

    /// The profile resolved to the chosen `shift`.
    #[must_use]
    pub fn chosen(&self, shift: u8) -> ChosenProfile {
        ChosenProfile {
            name_block_shift: shift,
            priority: self.priority,
            max_table_bytes: self.max_table_bytes,
            max_delta_bytes: self.max_delta_bytes,
            candidates: self.resolved_candidates(),
        }
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self::new(Priority::Balanced, DEFAULT_MAX_TABLE_BYTES)
    }
}

/// The subset of a [`Profile`] that describes a finished training run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChosenProfile {
    /// The block shift the image was encoded with.
    pub name_block_shift: u8,
    /// The rule that picked it.
    pub priority: Priority,
    /// The budget the image was checked against.
    pub max_table_bytes: usize,
    /// The tolerated growth over the baseline.
    pub max_delta_bytes: usize,
    /// Every candidate the run measured.
    pub candidates: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_parse_case_insensitively() {
        assert_eq!(Priority::parse("size"), Some(Priority::Size));
        assert_eq!(Priority::parse("BALANCED"), Some(Priority::Balanced));
        assert_eq!(Priority::parse("Latency"), Some(Priority::Latency));
        assert_eq!(Priority::parse("fast"), None);
        assert_eq!(Priority::Size.as_str(), "size");
    }

    #[test]
    fn default_profile_searches_every_candidate() {
        let profile = Profile::default();
        assert_eq!(profile.priority, Priority::Balanced);
        assert_eq!(profile.max_table_bytes, DEFAULT_MAX_TABLE_BYTES);
        assert_eq!(profile.resolved_candidates(), DEFAULT_CANDIDATE_SHIFTS);
        assert!(profile.baseline.is_none());
        assert_eq!(profile.max_delta_bytes, 0);
    }

    #[test]
    fn candidates_are_sorted_and_deduplicated() {
        let profile = Profile::default().with_candidates([8, 3, 5, 3]);
        assert_eq!(profile.resolved_candidates(), [3, 5, 8]);
    }

    #[test]
    fn a_fixed_shift_wins_over_the_candidate_list() {
        let profile = Profile::default()
            .with_candidates([3, 4, 8])
            .with_block_shift(7);
        assert_eq!(profile.resolved_candidates(), [7]);
        let chosen = profile.chosen(7);
        assert_eq!(chosen.name_block_shift, 7);
        assert_eq!(chosen.candidates, [7]);
    }

    #[test]
    fn baseline_and_budget_are_kept_in_the_chosen_profile() {
        let profile = Profile::new(Priority::Size, 12_345)
            .with_baseline(vec![0; 99], 17)
            .with_max_delta_bytes(23);
        let chosen = profile.chosen(6);
        assert_eq!(chosen.priority, Priority::Size);
        assert_eq!(chosen.max_table_bytes, 12_345);
        assert_eq!(chosen.max_delta_bytes, 23);
        assert_eq!(profile.baseline.as_deref(), Some([0; 99].as_slice()));
    }
}
