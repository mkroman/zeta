//! End-to-end training for the Nomina image.
//!
//! [`train`] parses the vendored inputs through the [`crate::nomina`] encoders and measures
//! every name-block candidate a [`Profile`] asks for. The emoji, general-category and
//! pattern sections do not depend on the name layout, so they are encoded once; only the name
//! section and the final packing run per candidate. Candidates that miss the budget are
//! dropped, [`Profile::priority`] then picks one image among the fitting ones, [`verify`]
//! re-decodes its name section, and the optional baseline is checked before [`TrainOutput`]
//! is returned.
//!
//! Training is deterministic: the same [`Inputs`] and profile always produce byte-identical
//! images and reports. Nothing reads the clock or the filesystem.

use std::fmt::Write as _;

use bytes::Buf;
use thiserror::Error;

use crate::nomina::{emoji, gc, image, names, patterns};
use crate::profile::{
    BALANCED_MAX_SHIFT, ChosenProfile, MAX_CANDIDATE_SHIFT, MIN_CANDIDATE_SHIFT, Priority, Profile,
};
use crate::ucd::Inputs;

/// A training run that cannot produce a usable image.
#[derive(Debug, Error)]
pub enum Error {
    /// The profile has no block shift to measure.
    #[error("the training profile has no name-block candidates")]
    NoCandidates,
    /// A candidate (or the fixed shift) is outside the supported range.
    #[error("name-block shift {shift} is outside {min}..={max}")]
    Shift {
        /// The offending shift.
        shift: u8,
        /// The smallest supported shift.
        min: u8,
        /// The largest supported shift.
        max: u8,
    },
    /// The packed image does not have the documented header and section layout.
    #[error("the Nomina image is malformed: {reason}")]
    Image {
        /// Why the image was rejected.
        reason: &'static str,
    },
    /// No candidate fits the profile's budget; the reported image is the smallest candidate.
    #[error("the image is {total_bytes} B, over the {max_bytes} B budget (block shift {shift})")]
    Budget {
        /// The block shift the chosen image used.
        shift: u8,
        /// The chosen image size.
        total_bytes: usize,
        /// The configured budget.
        max_bytes: usize,
    },
    /// The chosen image grew past the baseline plus the allowed delta.
    #[error(
        "baseline regression: {total_bytes} B is over the {baseline_bytes} B baseline plus \
         the {max_delta_bytes} B allowance"
    )]
    BaselineRegression {
        /// The chosen image size.
        total_bytes: usize,
        /// The size of the supplied baseline image.
        baseline_bytes: usize,
        /// The tolerated growth.
        max_delta_bytes: usize,
    },
}

/// One section of a packed Nomina image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionRecord {
    /// The section id (name, emoji, general category or patterns).
    pub id: usize,
    /// The manifest name of the section.
    pub name: &'static str,
    /// The section's byte offset in the image.
    pub offset: usize,
    /// The section's payload length.
    pub size: usize,
}

/// The validated layout of a Nomina image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageLayout {
    /// The Unicode version from the image header.
    pub version: (u8, u8, u8),
    /// The number of names per front-coded block, as a power of two.
    pub name_block_shift: u8,
    /// The name-section layout id (`0` block-local, `1` global checkpointed).
    pub name_layout: u8,
    /// The general-category page shift from the image header.
    pub gc_page_shift: u8,
    /// The four sections, in image order.
    pub sections: Vec<SectionRecord>,
    /// The total image size.
    pub total_bytes: usize,
}

impl ImageLayout {
    /// Parses and validates the 64-byte Nomina header and its section directory.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Image`] when the magic, section count, shifts, alignment, bounds or
    /// ordering are invalid, or when the last section does not end at the end of the image.
    pub fn parse(image: &[u8]) -> Result<Self, Error> {
        if image.len() < image::HEADER_LEN {
            return Err(Error::Image {
                reason: "the header is truncated",
            });
        }
        if &image[..4] != image::MAGIC {
            return Err(Error::Image {
                reason: "the magic is not NOM1",
            });
        }
        if usize::from(image[10]) != image::SECTION_COUNT {
            return Err(Error::Image {
                reason: "the section count is not four",
            });
        }
        let name_layout = image[13];
        if names::NameLayout::from_id(name_layout).is_none() {
            return Err(Error::Image {
                reason: "the name layout byte is unknown",
            });
        }
        let name_block_shift = image[11];
        if !(names::MIN_BLOCK_SHIFT..=names::MAX_BLOCK_SHIFT).contains(&name_block_shift) {
            return Err(Error::Image {
                reason: "the name block shift is outside the supported range",
            });
        }
        let gc_page_shift = image[12];
        if gc_page_shift != image::GC_PAGE_SHIFT {
            return Err(Error::Image {
                reason: "the general-category page shift is not eleven",
            });
        }

        Ok(Self {
            version: parse_version(image)?,
            name_block_shift,
            name_layout,
            gc_page_shift,
            sections: parse_sections(image)?,
            total_bytes: image.len(),
        })
    }
}

/// Reads the three `u16` version fields from the image header.
fn parse_version(image: &[u8]) -> Result<(u8, u8, u8), Error> {
    let mut version_bytes = image.get(4..10).ok_or(Error::Image {
        reason: "the version is truncated",
    })?;
    let major = version_bytes.try_get_u16_le().map_err(|_| Error::Image {
        reason: "the version is truncated",
    })?;
    let minor = version_bytes.try_get_u16_le().map_err(|_| Error::Image {
        reason: "the version is truncated",
    })?;
    let patch = version_bytes.try_get_u16_le().map_err(|_| Error::Image {
        reason: "the version is truncated",
    })?;
    Ok((
        u8::try_from(major).map_err(|_| Error::Image {
            reason: "the major version does not fit in a byte",
        })?,
        u8::try_from(minor).map_err(|_| Error::Image {
            reason: "the minor version does not fit in a byte",
        })?,
        u8::try_from(patch).map_err(|_| Error::Image {
            reason: "the patch version does not fit in a byte",
        })?,
    ))
}

/// Reads and validates the four section directory entries.
fn parse_sections(image: &[u8]) -> Result<Vec<SectionRecord>, Error> {
    let mut sections = Vec::with_capacity(image::SECTION_COUNT);
    let mut previous_end = image::HEADER_LEN;
    for id in 0..image::SECTION_COUNT {
        let at = 14 + id * 8;
        let mut entry = image.get(at..at + 8).ok_or(Error::Image {
            reason: "a section directory entry is truncated",
        })?;
        let offset = usize::try_from(entry.try_get_u32_le().map_err(|_| Error::Image {
            reason: "a section offset is truncated",
        })?)
        .map_err(|_| Error::Image {
            reason: "a section offset does not fit in usize",
        })?;
        let size = usize::try_from(entry.try_get_u32_le().map_err(|_| Error::Image {
            reason: "a section length is truncated",
        })?)
        .map_err(|_| Error::Image {
            reason: "a section length does not fit in usize",
        })?;
        if !offset.is_multiple_of(8) {
            return Err(Error::Image {
                reason: "a section offset is not eight-byte aligned",
            });
        }
        if offset < previous_end {
            return Err(Error::Image {
                reason: "section payloads overlap or are out of order",
            });
        }
        let end = offset.checked_add(size).ok_or(Error::Image {
            reason: "a section end overflows usize",
        })?;
        if end > image.len() {
            return Err(Error::Image {
                reason: "a section lies outside the image",
            });
        }
        sections.push(SectionRecord {
            id,
            name: section_name(id),
            offset,
            size,
        });
        previous_end = end;
    }
    if previous_end != image.len() {
        return Err(Error::Image {
            reason: "the image has trailing bytes",
        });
    }
    Ok(sections)
}

/// The statistic and size breakdown of the chosen name section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameStats {
    /// Names in the section.
    pub name_count: usize,
    /// Front-coded blocks.
    pub block_count: usize,
    /// Maximal runs of consecutive named code points.
    pub run_count: usize,
    /// The block shift the section was encoded with.
    pub block_shift: u8,
    /// The name layout the section was encoded with.
    pub layout: names::NameLayout,
    /// Header, page directory, runs and their padding.
    pub index_bytes: usize,
    /// The `u32` bit offset of every block (block-local layout only).
    pub offset_bytes: usize,
    /// The checkpoint offset directory (global layout only).
    pub checkpoint_directory_bytes: usize,
    /// The checkpoint bit offsets and predecessor names (global layout only).
    pub checkpoint_bytes: usize,
    /// The context map, model directory and model blobs.
    pub model_bytes: usize,
    /// The context map and model directory inside [`Self::model_bytes`].
    pub model_directory_bytes: usize,
    /// The Huffman bitstream.
    pub stream_bytes: usize,
    /// The exact bit length of [`Self::stream_bytes`] before its final-byte padding.
    pub stream_bits: usize,
    /// The number of byte contexts, including the start-of-name context.
    pub context_count: usize,
}

impl NameStats {
    /// The total section size in bytes.
    #[must_use]
    pub const fn total_bytes(&self) -> usize {
        // Checkpoint bits are part of the main stream, so they are reported separately but
        // must not be added here.
        self.index_bytes + self.offset_bytes + self.model_bytes + self.stream_bytes
    }
}

/// One measured candidate in a training run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The candidate block shift.
    pub name_block_shift: u8,
    /// The encoded name-section size.
    pub name_bytes: usize,
    /// The packed image size with this name section.
    pub total_bytes: usize,
    /// Whether the profile chose this candidate.
    pub chosen: bool,
}

/// What each stage round-tripped.
///
/// Every flag is backed by a check that actually runs before a [`TrainOutput`] is returned:
/// the builders and the name verifier `assert!` on any mismatch, so a failed round-trip
/// aborts training with a panic rather than reporting `false`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the five checks are independent facts reported in the manifest"
)]
pub struct Verification {
    /// The chosen name section decoded back to every input name, in [`verify`].
    pub names: bool,
    /// The emoji streams decoded back to every entry; enforced by the emoji builder's
    /// internal round-trip assertions.
    pub emoji: bool,
    /// The category streams rebuilt into every range; enforced by the general-category
    /// builder's internal round-trip assertions.
    pub gc: bool,
    /// Every pattern range and dictionary entry decoded back; enforced by the pattern
    /// builder's internal round-trip assertions.
    pub patterns: bool,
    /// The image header and section directory validated by [`ImageLayout::parse`] in
    /// [`verify`].
    pub image: bool,
}

impl Verification {
    /// Whether every stage verified.
    #[must_use]
    pub const fn verified(&self) -> bool {
        self.names && self.emoji && self.gc && self.patterns && self.image
    }
}

/// The result of one successful training run.
#[derive(Debug)]
pub struct TrainOutput {
    /// The complete Nomina image, ready to write.
    pub image: Vec<u8>,
    /// The Unicode version from the inputs and the image header.
    pub version: (u8, u8, u8),
    /// The Emoji version from `emoji-test.txt`.
    pub emoji_version: (u8, u8),
    /// The profile resolved to the chosen block shift.
    pub profile: ChosenProfile,
    /// The section records of the chosen image.
    pub sections: Vec<SectionRecord>,
    /// The breakdown of the chosen name section.
    pub names: NameStats,
    /// Emoji section statistics.
    pub emoji: emoji::EmojiStats,
    /// General-category section statistics.
    pub gc: gc::GcStats,
    /// Pattern section statistics.
    pub patterns: patterns::PatternStats,
    /// Every candidate this run measured, in ascending shift order.
    pub candidates: Vec<Candidate>,
    /// The candidate this run chose; also present in [`Self::candidates`].
    pub chosen: Candidate,
    /// The size of [`Self::image`].
    pub total_bytes: usize,
    /// A compact, deterministic table of the candidates and the chosen profile.
    pub report: String,
    /// The round-trip checks this run performed.
    pub verification: Verification,
    /// The baseline comparison, when a baseline was supplied.
    pub baseline: Option<BaselineComparison>,
}

/// The measured relationship between a trained image and its baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BaselineComparison {
    /// The baseline image size.
    pub baseline_bytes: usize,
    /// The trained image size.
    pub total_bytes: usize,
    /// The tolerated growth.
    pub max_delta_bytes: usize,
}

/// Trains one Nomina image from already parsed inputs.
///
/// Emoji, general-category and pattern sections are encoded and verified once. Each candidate
/// block shift then encodes the name section, packs a complete image and records its size;
/// candidates over [`Profile::max_table_bytes`] are dropped and the priority picks one of the
/// rest. The chosen name section is re-decoded against the inputs, the image layout is
/// validated, and the baseline is enforced.
///
/// # Errors
///
/// Returns [`Error::NoCandidates`] for an empty candidate list, [`Error::Shift`] for a shift
/// outside 3..=8, [`Error::Budget`] when no candidate fits
/// [`Profile::max_table_bytes`] (a pinned [`Profile::name_block_shift`] has no alternative,
/// so it keeps the hard error), [`Error::Image`] for an inconsistent packed image, and
/// [`Error::BaselineRegression`] when the chosen image grows past the baseline allowance.
///
/// # Panics
///
/// Panics on malformed vendored data, as the encoders do: a bad record is a build failure.
/// The name verifier also panics if the chosen section does not decode back to every input
/// name.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
///
/// use zeta_unicode_build::{Profile, read_inputs, train};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let inputs = read_inputs(Path::new("../zeta-unicode/data"))?;
/// let output = train(&inputs, &Profile::default())?;
/// std::fs::write("nomina.bin", &output.image)?;
/// # Ok(())
/// # }
/// ```
#[allow(
    clippy::too_many_lines,
    reason = "the candidate loop, verification and budget checks are one linear pipeline"
)]
pub fn train(inputs: &Inputs, profile: &Profile) -> Result<TrainOutput, Error> {
    let shifts = profile.resolved_candidates();
    if shifts.is_empty() {
        return Err(Error::NoCandidates);
    }
    for &shift in &shifts {
        if !(MIN_CANDIDATE_SHIFT..=MAX_CANDIDATE_SHIFT).contains(&shift) {
            return Err(Error::Shift {
                shift,
                min: MIN_CANDIDATE_SHIFT,
                max: MAX_CANDIDATE_SHIFT,
            });
        }
    }

    let (emoji_section, emoji_stats) =
        emoji::build(&inputs.emoji, &inputs.sequences, &inputs.names);
    let (gc_section, gc_stats) = gc::build(&inputs.gc);
    let (pattern_section, pattern_stats) = patterns::build(&inputs.names);
    assert_eq!(emoji_stats.section_bytes, emoji_section.len());
    assert_eq!(gc_stats.section_bytes, gc_section.len());
    assert_eq!(pattern_stats.section_bytes, pattern_section.len());

    let mut builds: Vec<CandidateBuild> = Vec::with_capacity(shifts.len());
    for &shift in &shifts {
        let encoded = names::encode_global_with_block_shift(inputs, shift);
        let packed = image::pack_with_layout(
            [
                encoded.bytes.as_slice(),
                emoji_section.as_slice(),
                gc_section.as_slice(),
                pattern_section.as_slice(),
            ],
            inputs.version,
            shift,
            image::GC_PAGE_SHIFT,
            encoded.layout.id(),
        );
        builds.push(CandidateBuild {
            shift,
            encoded,
            image: packed,
        });
    }
    let measured: Vec<Measurement> = builds.iter().map(CandidateBuild::measurement).collect();

    // Only candidates that fit the budget may be chosen; a pinned shift has no alternative
    // and keeps the hard error through the empty `fitting` set.
    let chosen_index = choose_index(profile.priority, &measured, profile.max_table_bytes)
        .ok_or_else(|| {
            let smallest = measured
                .iter()
                .min_by_key(|measurement| (measurement.total_bytes, measurement.shift))
                .expect("training always measures at least one candidate");
            Error::Budget {
                shift: smallest.shift,
                total_bytes: smallest.total_bytes,
                max_bytes: profile.max_table_bytes,
            }
        })?;
    let candidates: Vec<Candidate> = builds
        .iter()
        .enumerate()
        .map(|(index, build)| Candidate {
            name_block_shift: build.shift,
            name_bytes: build.encoded.bytes.len(),
            total_bytes: build.image.len(),
            chosen: index == chosen_index,
        })
        .collect();
    let chosen = candidates
        .get(chosen_index)
        .copied()
        .expect("the chosen index came from this candidate list");

    let CandidateBuild { encoded, image, .. } = builds.swap_remove(chosen_index);
    let layout = verify(&image, inputs)?;
    let name_stats = NameStats::from_encoded(&encoded);
    assert_eq!(
        name_stats.total_bytes(),
        encoded.bytes.len(),
        "the name breakdown must sum to the section size"
    );

    assert!(
        layout.total_bytes <= profile.max_table_bytes,
        "the chosen candidate was filtered to fit the budget"
    );
    let baseline = match &profile.baseline {
        Some(baseline) => {
            let allowed = baseline.len().saturating_add(profile.max_delta_bytes);
            if layout.total_bytes > allowed {
                return Err(Error::BaselineRegression {
                    total_bytes: layout.total_bytes,
                    baseline_bytes: baseline.len(),
                    max_delta_bytes: profile.max_delta_bytes,
                });
            }
            Some(BaselineComparison {
                baseline_bytes: baseline.len(),
                total_bytes: layout.total_bytes,
                max_delta_bytes: profile.max_delta_bytes,
            })
        }
        None => None,
    };

    let chosen_profile = profile.chosen(chosen.name_block_shift);
    let report = render_report(&chosen_profile, &candidates, &layout);
    // Every flag is backed by a check that ran above or inside the builders: `verify`
    // re-decoded the chosen name section and validated the layout, and the emoji,
    // general-category and pattern builders assert their internal round-trips.
    let verification = Verification {
        names: true,
        emoji: true,
        gc: true,
        patterns: true,
        image: true,
    };
    Ok(TrainOutput {
        total_bytes: layout.total_bytes,
        image,
        version: inputs.version,
        emoji_version: inputs.emoji_version,
        profile: chosen_profile,
        sections: layout.sections,
        names: name_stats,
        emoji: emoji_stats,
        gc: gc_stats,
        patterns: pattern_stats,
        candidates,
        chosen,
        report,
        verification,
        baseline,
    })
}

/// Re-validates a packed image and re-decodes its name section against `inputs`.
///
/// # Errors
///
/// Returns [`Error::Image`] when the layout is malformed or its version differs from the
/// inputs' version.
///
/// # Panics
///
/// Panics when the name section does not decode back to every input name: a mismatch is a
/// build failure, not a runtime condition.
pub fn verify(image: &[u8], inputs: &Inputs) -> Result<ImageLayout, Error> {
    let layout = ImageLayout::parse(image)?;
    if layout.version != inputs.version {
        return Err(Error::Image {
            reason: "the image version does not match the inputs",
        });
    }
    let names_section = layout.sections.get(image::NAMES).ok_or(Error::Image {
        reason: "the image has no name section",
    })?;
    let section = names_section
        .offset
        .checked_add(names_section.size)
        .and_then(|end| image.get(names_section.offset..end))
        .ok_or(Error::Image {
            reason: "the name section is outside the image",
        })?;
    match layout.name_layout {
        image::LAYOUT_BLOCK_LOCAL => {
            names::verify_with_block_shift(section, inputs, layout.name_block_shift);
        }
        image::LAYOUT_GLOBAL => {
            names::verify_global_with_block_shift(section, inputs, layout.name_block_shift);
        }
        _ => {
            return Err(Error::Image {
                reason: "the name layout byte is unknown",
            });
        }
    }
    Ok(layout)
}

impl NameStats {
    /// Copies the breakdown out of an encoded name section.
    const fn from_encoded(encoded: &names::Encoded) -> Self {
        Self {
            name_count: encoded.name_count,
            block_count: encoded.block_count,
            run_count: encoded.run_count,
            block_shift: encoded.block_shift,
            layout: encoded.layout,
            index_bytes: encoded.index_bytes,
            offset_bytes: encoded.offset_bytes,
            checkpoint_directory_bytes: encoded.checkpoint_directory_bytes,
            checkpoint_bytes: encoded.checkpoint_bytes,
            model_bytes: encoded.model_bytes,
            model_directory_bytes: encoded.model_directory_bytes,
            stream_bytes: encoded.stream_bytes,
            stream_bits: encoded.stream_bits,
            context_count: encoded.context_count,
        }
    }
}

/// One candidate's size, before the priority rule picks a winner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Measurement {
    shift: u8,
    total_bytes: usize,
}

/// One measured candidate: its shift, its encoded name section and its packed image.
struct CandidateBuild {
    /// The name-block shift the candidate was encoded with.
    shift: u8,
    /// The encoded name section and its size breakdown.
    encoded: names::Encoded,
    /// The complete image with this candidate's name section.
    image: Vec<u8>,
}

impl CandidateBuild {
    /// The candidate's row in the selection table.
    const fn measurement(&self) -> Measurement {
        Measurement {
            shift: self.shift,
            total_bytes: self.image.len(),
        }
    }
}

/// Picks the candidate index for `priority` among the candidates that fit
/// `max_table_bytes`; ties always go to the lower shift.
///
/// Returns `None` when no candidate fits; the caller then reports the smallest candidate.
fn choose_index(
    priority: Priority,
    measurements: &[Measurement],
    max_table_bytes: usize,
) -> Option<usize> {
    assert!(
        !measurements.is_empty(),
        "training always measures at least one candidate"
    );
    let fitting = || {
        measurements
            .iter()
            .enumerate()
            .filter(|(_, measurement)| measurement.total_bytes <= max_table_bytes)
    };
    match priority {
        Priority::Latency => fitting()
            .min_by_key(|(_, measurement)| measurement.shift)
            .map(|(index, _)| index),
        Priority::Size => smallest_index(fitting()),
        Priority::Balanced => smallest_index(
            fitting().filter(|(_, measurement)| measurement.shift <= BALANCED_MAX_SHIFT),
        )
        .or_else(|| smallest_index(fitting())),
    }
}

/// The index of the smallest `(total, shift)` pair in `measurements`.
fn smallest_index<'a>(
    measurements: impl Iterator<Item = (usize, &'a Measurement)>,
) -> Option<usize> {
    measurements
        .min_by_key(|(_, measurement)| (measurement.total_bytes, measurement.shift))
        .map(|(index, _)| index)
}

/// Renders the deterministic candidate table shown by the CLI and stored in [`TrainOutput`].
fn render_report(chosen: &ChosenProfile, candidates: &[Candidate], layout: &ImageLayout) -> String {
    let mut out = String::with_capacity(1_024);
    let _ = writeln!(
        out,
        "Nomina training: priority {}, budget {} B",
        chosen.priority, chosen.max_table_bytes
    );
    let _ = writeln!(
        out,
        "{:>5} {:>10} {:>10}  chosen",
        "shift", "names", "total"
    );
    for candidate in candidates {
        let _ = writeln!(
            out,
            "{:>5} {:>10} {:>10}  {}",
            candidate.name_block_shift,
            candidate.name_bytes,
            candidate.total_bytes,
            if candidate.chosen { "*" } else { "" }
        );
    }
    let _ = writeln!(
        out,
        "chosen: block shift {} ({} names per block, {}), total {} B",
        chosen.name_block_shift,
        1usize << chosen.name_block_shift,
        if layout.name_layout == image::LAYOUT_GLOBAL {
            "global checkpointed"
        } else {
            "block-local"
        },
        layout.total_bytes
    );
    let payload: usize = layout.sections.iter().map(|section| section.size).sum();
    let _ = writeln!(
        out,
        "sections: names {}, emoji {}, general category {}, patterns {}; \
         header and padding {} B",
        layout.sections[image::NAMES].size,
        layout.sections[image::EMOJI].size,
        layout.sections[image::GC].size,
        layout.sections[image::PATTERNS].size,
        layout.total_bytes - payload
    );
    out
}

/// The stable manifest name of a Nomina section id.
const fn section_name(id: usize) -> &'static str {
    match id {
        image::NAMES => "names",
        image::EMOJI => "emoji",
        image::GC => "general_category",
        image::PATTERNS => "name_patterns",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::profile::DEFAULT_MAX_TABLE_BYTES;

    /// Reads the vendored 18.0.0 inputs.
    fn inputs() -> Inputs {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zeta-unicode/data");
        crate::ucd::read_inputs(&data).expect("vendored inputs")
    }

    /// A synthetic measurement row.
    const fn measurement(shift: u8, total_bytes: usize) -> Measurement {
        Measurement { shift, total_bytes }
    }

    #[test]
    fn size_picks_the_smallest_total_and_ties_toward_the_lower_shift() {
        let rows = [
            measurement(3, 100),
            measurement(4, 90),
            measurement(5, 90),
            measurement(6, 95),
        ];
        assert_eq!(choose_index(Priority::Size, &rows, usize::MAX), Some(1));
    }

    #[test]
    fn latency_picks_the_lowest_shift() {
        let rows = [measurement(5, 10), measurement(3, 100), measurement(4, 20)];
        assert_eq!(choose_index(Priority::Latency, &rows, usize::MAX), Some(1));
    }

    #[test]
    fn balanced_stays_at_or_below_six_and_falls_back_when_it_must() {
        let rows = [
            measurement(3, 100),
            measurement(4, 80),
            measurement(5, 70),
            measurement(6, 60),
            measurement(7, 50),
        ];
        assert_eq!(choose_index(Priority::Balanced, &rows, usize::MAX), Some(3));
        let large = [measurement(7, 50), measurement(8, 40)];
        assert_eq!(
            choose_index(Priority::Balanced, &large, usize::MAX),
            Some(1)
        );
        let single = [measurement(8, 40)];
        assert_eq!(
            choose_index(Priority::Balanced, &single, usize::MAX),
            Some(0)
        );
    }

    #[test]
    fn candidates_over_the_budget_are_never_chosen() {
        let rows = [
            measurement(5, 300),
            measurement(6, 200),
            measurement(7, 150),
            measurement(8, 120),
        ];
        // Only shifts 7 and 8 fit a 150 B budget: latency takes the lowest fitting shift,
        // balanced falls back to the smallest fitting image, size takes the smallest.
        assert_eq!(choose_index(Priority::Latency, &rows, 150), Some(2));
        assert_eq!(choose_index(Priority::Balanced, &rows, 150), Some(3));
        assert_eq!(choose_index(Priority::Size, &rows, 150), Some(3));
        // A 199 B budget fits shifts 7 and 8: size takes 8, latency still takes 7.
        assert_eq!(choose_index(Priority::Size, &rows, 199), Some(3));
        assert_eq!(choose_index(Priority::Latency, &rows, 199), Some(2));
        assert_eq!(choose_index(Priority::Latency, &rows, 119), None);
        assert_eq!(choose_index(Priority::Size, &rows, 119), None);
        assert_eq!(choose_index(Priority::Balanced, &rows, 119), None);
    }

    #[test]
    fn a_budget_between_candidate_sizes_selects_a_fitting_candidate() {
        let inputs = inputs();
        let balanced = Profile::new(Priority::Balanced, 185_000);
        let output = train(&inputs, &balanced).expect("a fitting candidate exists");
        assert_eq!(output.chosen.name_block_shift, 8);
        assert_eq!(output.chosen.total_bytes, 181_026);
        assert_eq!(output.total_bytes, 181_026);
        assert!(
            output.report.contains("chosen: block shift 8"),
            "{}",
            output.report
        );

        let latency = Profile::new(Priority::Latency, 190_000);
        let output = train(&inputs, &latency).expect("a fitting candidate exists");
        // Shift 5 misses the 190,000 B budget, so latency takes the lowest shift that fits.
        assert_eq!(output.chosen.name_block_shift, 6);

        let size = Profile::new(Priority::Size, 185_000);
        let output = train(&inputs, &size).expect("a fitting candidate exists");
        assert_eq!(output.chosen.name_block_shift, 8);
        assert_eq!(output.chosen.total_bytes, 181_026);
    }

    #[test]
    fn an_impossible_budget_reports_the_smallest_candidate() {
        let inputs = inputs();
        let error = train(&inputs, &Profile::new(Priority::Balanced, 1_000))
            .expect_err("no candidate fits 1,000 bytes");
        match error {
            Error::Budget {
                shift,
                total_bytes,
                max_bytes,
            } => {
                // Shift 8 packs the smallest image, so it is the one reported.
                assert_eq!(shift, 8);
                assert_eq!(total_bytes, 181_026);
                assert_eq!(max_bytes, 1_000);
            }
            other => panic!("expected Budget, got {other:?}"),
        }
    }

    #[test]
    fn trains_the_vendored_data_within_budget() {
        let inputs = inputs();
        let output = train(&inputs, &Profile::default()).expect("training succeeds");

        assert_eq!(output.version, (18, 0, 0));
        assert_eq!(output.emoji_version, (18, 0));
        assert!(output.total_bytes <= DEFAULT_MAX_TABLE_BYTES);
        assert_eq!(output.total_bytes, output.image.len());
        assert_eq!(output.chosen.total_bytes, output.total_bytes);
        assert_eq!(
            output.chosen.name_block_shift,
            output.profile.name_block_shift
        );
        assert!(output.verification.verified());
        assert_eq!(
            output.candidates.len(),
            crate::profile::DEFAULT_CANDIDATE_SHIFTS.len()
        );
        assert_eq!(
            output
                .candidates
                .iter()
                .filter(|candidate| candidate.chosen)
                .count(),
            1
        );

        let layout = ImageLayout::parse(&output.image).expect("valid layout");
        assert_eq!(layout.sections, output.sections);
        assert_eq!(layout.total_bytes, output.total_bytes);
        assert_eq!(layout.sections[image::NAMES].size, output.chosen.name_bytes);
        assert_eq!(
            layout.sections[image::NAMES].size,
            output.names.total_bytes()
        );
        assert_eq!(output.names.name_count, 35_416);
        assert!(
            output.report.contains(&format!(
                "chosen: block shift {}",
                output.profile.name_block_shift
            )),
            "report: {}",
            output.report
        );
        assert!(
            output.report.contains(&format!(
                "sections: names {}, emoji {}, general category {}, patterns {};",
                layout.sections[image::NAMES].size,
                layout.sections[image::EMOJI].size,
                layout.sections[image::GC].size,
                layout.sections[image::PATTERNS].size
            )),
            "report: {}",
            output.report
        );
        for candidate in &output.candidates {
            let line = format!(
                "{:>5} {:>10} {:>10}",
                candidate.name_block_shift, candidate.name_bytes, candidate.total_bytes
            );
            assert!(output.report.contains(&line), "missing {line:?}");
        }
    }

    #[test]
    fn a_second_run_is_byte_identical() {
        let inputs = inputs();
        let profile = Profile::default();
        let first = train(&inputs, &profile).expect("first run");
        let second = train(&inputs, &profile).expect("second run");
        assert_eq!(first.image, second.image, "the image must be deterministic");
        assert_eq!(
            first.report, second.report,
            "the report must be deterministic"
        );
        assert_eq!(first.candidates, second.candidates);
        assert_eq!(first.names, second.names);
    }

    #[test]
    fn every_candidate_shift_round_trips_and_has_valid_section_bounds() {
        let inputs = inputs();
        for shift in MIN_CANDIDATE_SHIFT..=MAX_CANDIDATE_SHIFT {
            // Shift 3 is larger than the default budget; this test is about the format, not
            // the size policy.
            let profile = Profile::new(Priority::Balanced, usize::MAX).with_block_shift(shift);
            let output = train(&inputs, &profile).expect("candidate training");
            assert_eq!(output.profile.name_block_shift, shift);
            assert_eq!(output.candidates.len(), 1);
            assert_eq!(output.chosen.name_block_shift, shift);
            assert_eq!(output.names.block_shift, shift);

            let layout = ImageLayout::parse(&output.image).expect("valid layout");
            assert_eq!(layout.name_block_shift, shift);
            let mut previous_end = image::HEADER_LEN;
            for section in &layout.sections {
                assert!(section.offset >= previous_end);
                assert_eq!(section.offset % 8, 0);
                assert_eq!(
                    &output.image[section.offset..section.offset + section.size].len(),
                    &section.size
                );
                previous_end = section.offset + section.size;
            }
            assert_eq!(previous_end, output.image.len());
            names::verify_global_with_block_shift(
                &output.image[layout.sections[image::NAMES].offset
                    ..layout.sections[image::NAMES].offset + layout.sections[image::NAMES].size],
                &inputs,
                shift,
            );
        }
    }

    #[test]
    fn the_nomina_manifest_is_deterministic_and_relative() {
        let data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zeta-unicode/data");
        let inputs = crate::ucd::read_inputs(&data_dir).expect("vendored inputs");
        let output =
            train(&inputs, &Profile::default().with_block_shift(8)).expect("training succeeds");
        let out = tempfile::tempdir().expect("out dir");

        crate::manifest::write_manifest(&data_dir, out.path(), &inputs, &output)
            .expect("first manifest");
        let first = std::fs::read(out.path().join("manifest.json")).expect("first read");
        crate::manifest::write_manifest(&data_dir, out.path(), &inputs, &output)
            .expect("second manifest");
        let second = std::fs::read(out.path().join("manifest.json")).expect("second read");
        assert_eq!(first, second, "the manifest must not depend on the run");

        let text = String::from_utf8(first).expect("UTF-8 manifest");
        assert!(text.contains("\"layout\": \"nomina\""));
        assert!(text.contains("\"unicode_version\": [18, 0, 0]"));
        assert!(text.contains("\"emoji_version\": [18, 0]"));
        assert!(text.contains(&format!("\"total_bytes\": {}", output.total_bytes)));
        assert!(text.contains("\"name_block_shift\": 8"));
        assert!(text.contains("\"path\": \"DerivedName.txt\""));
        assert!(text.contains(&format!(
            "\"name_block_shift\": 8, \"name_bytes\": {}",
            output.names.total_bytes()
        )));
        assert!(text.contains(&format!(
            "\"offset\": {}, \"size\": {}",
            output.sections[0].offset, output.sections[0].size
        )));
        assert!(text.contains("\"status\": \"verified\""));
        assert!(
            !text.contains("derived-names.rules"),
            "the Nomina manifest has no rules file"
        );
        assert!(
            !text.contains(env!("CARGO_MANIFEST_DIR")),
            "an absolute path leaked into the manifest"
        );
    }

    #[test]
    fn a_tight_budget_is_a_structured_error() {
        let inputs = inputs();
        let profile = Profile::new(Priority::Balanced, 1_000).with_block_shift(3);
        let error = train(&inputs, &profile).expect_err("the budget is too small");
        match error {
            Error::Budget {
                shift,
                total_bytes,
                max_bytes,
            } => {
                assert_eq!(shift, 3);
                assert!(total_bytes > 1_000);
                assert_eq!(max_bytes, 1_000);
            }
            other => panic!("expected Budget, got {other:?}"),
        }
    }

    #[test]
    fn a_baseline_regression_is_a_structured_error() {
        let inputs = inputs();
        let profile = Profile::default().with_block_shift(8);
        let output = train(&inputs, &profile).expect("training succeeds");
        let baseline = output.image[..output.image.len() - 1].to_vec();
        let regressing = Profile::default()
            .with_block_shift(8)
            .with_baseline(baseline, 0);
        let error = train(&inputs, &regressing).expect_err("one byte over the baseline");
        match error {
            Error::BaselineRegression {
                total_bytes,
                baseline_bytes,
                max_delta_bytes,
            } => {
                assert_eq!(total_bytes, output.total_bytes);
                assert_eq!(baseline_bytes, output.total_bytes - 1);
                assert_eq!(max_delta_bytes, 0);
            }
            other => panic!("expected BaselineRegression, got {other:?}"),
        }
    }

    #[test]
    fn a_baseline_with_room_is_recorded() {
        let inputs = inputs();
        let profile = Profile::default().with_block_shift(8);
        let output = train(&inputs, &profile).expect("training succeeds");
        let baselined = Profile::default()
            .with_block_shift(8)
            .with_baseline(output.image.clone(), 32);
        let comparison = train(&inputs, &baselined)
            .expect("the same image fits the allowance")
            .baseline
            .expect("a baseline was supplied");
        assert_eq!(comparison.baseline_bytes, output.total_bytes);
        assert_eq!(comparison.total_bytes, output.total_bytes);
        assert_eq!(comparison.max_delta_bytes, 32);
    }

    #[test]
    fn a_fixed_shift_outside_the_candidate_range_is_rejected() {
        let inputs = inputs();
        for shift in [0, 1, 2, 9, 255] {
            let error = train(&inputs, &Profile::default().with_block_shift(shift))
                .expect_err("the shift is outside the candidate range");
            assert!(matches!(error, Error::Shift { shift: found, .. } if found == shift));
        }
    }

    #[test]
    fn an_empty_candidate_list_is_rejected() {
        let inputs = inputs();
        let profile = Profile::default().with_candidates([]);
        assert!(matches!(train(&inputs, &profile), Err(Error::NoCandidates)));
    }

    #[test]
    fn malformed_images_are_rejected() {
        assert!(ImageLayout::parse(&[]).is_err());
        assert!(ImageLayout::parse(b"NOM1").is_err());
        let inputs = inputs();
        let output =
            train(&inputs, &Profile::default().with_block_shift(8)).expect("training succeeds");

        let mut wrong_magic = output.image.clone();
        wrong_magic[0] = b'X';
        assert!(ImageLayout::parse(&wrong_magic).is_err());

        let mut wrong_count = output.image.clone();
        wrong_count[10] = 3;
        assert!(ImageLayout::parse(&wrong_count).is_err());

        let mut wrong_page = output.image.clone();
        wrong_page[12] = 10;
        assert!(ImageLayout::parse(&wrong_page).is_err());

        let mut wrong_layout = output.image.clone();
        wrong_layout[13] = 2;
        assert!(ImageLayout::parse(&wrong_layout).is_err());

        let mut out_of_bounds = output.image.clone();
        out_of_bounds[14..18].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(ImageLayout::parse(&out_of_bounds).is_err());

        let mut trailing = output.image;
        trailing.push(0);
        assert!(ImageLayout::parse(&trailing).is_err());
    }
}
