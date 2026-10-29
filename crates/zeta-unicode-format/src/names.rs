//! The name section's fixed index and Huffman model area.
//!
//! Both halves of the format read the same structural fields: the runtime builds its lookup
//! tables from them and the trainer's verifier replays the entries through the same index. This
//! module owns that structure, so a layout change is one edit. Only the entry reconstruction
//! itself stays duplicated: the build verifier keeps its own front-coding decoder as an
//! independent oracle.
//!
//! ```text
//! off  field
//!   0  u32 name_count
//!   4  u32 block_count                (ceil(name_count / 2^shift))
//!   8  u16 run_count
//!  10  u16 page_count                 (545)
//!  12  u16 page_directory[page_count]  first run whose end is strictly after page << 11
//!      run[run_count]                 { u32 first_cp, u16 length, u16 base_id }
//!      (padding to a u32 boundary)
//!      u32 block_bits[block_count]    MSB-first bit offset of each block
//!      model area (see [`NameModels`])
//!      the MSB-first Huffman bitstream
//! ```

use crate::huffman::Model;
use crate::layout::{
    MAX_CODE_BITS, MAX_NAME_BLOCK_SHIFT, MAX_NAME_MODELS, MIN_NAME_BLOCK_SHIFT,
    NAME_CONTEXT_MAP_LEN, NAME_INDEX_HEADER_LEN, NAME_MODEL_COUNT_LEN,
    NAME_MODEL_DIRECTORY_ENTRY_LEN, NAME_PAGE_COUNT, NAME_PAGE_DIRECTORY_ENTRY_LEN,
    NAME_PAGE_SHIFT, NAME_RUN_LEN, UNICODE_END,
};

/// Reads a little-endian `u16` at `at`.
fn read_u16(section: &[u8], at: usize) -> Option<u16> {
    let bytes: [u8; 2] = section.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_le_bytes(bytes))
}

/// Reads a little-endian `u32` at `at`.
fn read_u32(section: &[u8], at: usize) -> Option<u32> {
    let bytes: [u8; 4] = section.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

/// The validated fixed header, page directory, run records and block offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameIndex<'a> {
    /// Names in the section.
    pub name_count: usize,
    /// Number of front-coded blocks.
    pub block_count: usize,
    /// Number of run records.
    pub run_count: usize,
    /// First run index of every 2,048-code-point page.
    pub page_dir: &'a [u8],
    /// `{ u32 first_cp, u16 length, u16 base_id }` records.
    pub runs: &'a [u8],
    /// The `u32` bit offset of every name block.
    pub block_bits: &'a [u8],
    /// Offset of the model area from the start of the section.
    pub models_at: usize,
}

impl<'a> NameIndex<'a> {
    /// Parses and validates the fixed header, page directory, runs and block offsets.
    ///
    /// `block_shift` is the image header's name-block shift. Returns `None` for a truncated or
    /// inconsistent index: runs must be ordered, disjoint and in range, their base ids must
    /// tile the name ids, and the page directory must be a non-decreasing list of run indices.
    #[must_use]
    pub fn parse(section: &'a [u8], block_shift: u8) -> Option<Self> {
        if !(MIN_NAME_BLOCK_SHIFT..=MAX_NAME_BLOCK_SHIFT).contains(&block_shift) {
            return None;
        }
        let name_count = usize::try_from(read_u32(section, 0)?).ok()?;
        let block_count = usize::try_from(read_u32(section, 4)?).ok()?;
        let run_count = usize::from(read_u16(section, 8)?);
        let page_count = usize::from(read_u16(section, 10)?);
        if name_count == 0 || page_count != NAME_PAGE_COUNT || run_count == 0 {
            return None;
        }
        let block_size = 1usize << block_shift;
        if block_count != name_count.div_ceil(block_size) {
            return None;
        }

        let page_dir_at = NAME_INDEX_HEADER_LEN;
        let runs_at =
            page_dir_at.checked_add(page_count.checked_mul(NAME_PAGE_DIRECTORY_ENTRY_LEN)?)?;
        let runs_end = runs_at.checked_add(run_count.checked_mul(NAME_RUN_LEN)?)?;
        let starts_at = runs_end.checked_add(3)? & !3;
        let starts_end = starts_at.checked_add(block_count.checked_mul(4)?)?;
        if starts_end > section.len() {
            return None;
        }

        let page_dir = section.get(page_dir_at..runs_at)?;
        let runs = section.get(runs_at..runs_end)?;
        validate_index(page_dir, runs, run_count, name_count)?;

        Some(Self {
            name_count,
            block_count,
            run_count,
            page_dir,
            runs,
            block_bits: section.get(starts_at..starts_end)?,
            models_at: starts_end,
        })
    }

    /// Returns the name id of `cp`, or `None` when it has no stored name.
    #[must_use]
    pub fn id_of(&self, cp: u32) -> Option<u32> {
        if cp > UNICODE_END {
            return None;
        }
        let page = usize::try_from(cp >> NAME_PAGE_SHIFT).ok()?;
        let page_at = page.checked_mul(NAME_PAGE_DIRECTORY_ENTRY_LEN)?;
        let mut run = usize::from(read_u16(self.page_dir, page_at)?);
        loop {
            if run >= self.run_count {
                return None;
            }
            let at = run.checked_mul(NAME_RUN_LEN)?;
            let first = read_u32(self.runs, at)?;
            if cp < first {
                return None;
            }
            let length = u32::from(read_u16(self.runs, at.checked_add(4)?)?);
            if cp - first < length {
                let base = u32::from(read_u16(self.runs, at.checked_add(6)?)?);
                return base.checked_add(cp - first);
            }
            run += 1;
        }
    }
}

/// Checks the run records and page directory the encoder writes.
///
/// A run must start after the previous one ends (so the runs are strictly ordered and
/// disjoint), end inside the Unicode range and continue the name-id sequence at exactly
/// `base_id`; together the runs must cover all `name_count` ids. The page directory must be a
/// non-decreasing list of run indices at most `run_count`. Every image the encoder produces
/// satisfies these, and a section that does not is rejected before `id_of` can index outside
/// the run array or the name table or resolve to another run's name.
fn validate_index(page_dir: &[u8], runs: &[u8], run_count: usize, name_count: usize) -> Option<()> {
    let mut previous_end = None;
    let mut expected_base = 0usize;
    for record in runs.as_chunks::<NAME_RUN_LEN>().0 {
        let first = read_u32(record, 0)?;
        let length = u32::from(read_u16(record, 4)?);
        let base = usize::from(read_u16(record, 6)?);
        if length == 0 {
            return None;
        }
        let end = first.checked_add(length - 1)?;
        let length = usize::try_from(length).ok()?;
        if previous_end.is_some_and(|previous| first <= previous)
            || end > UNICODE_END
            || base != expected_base
            || base.checked_add(length)? > name_count
        {
            return None;
        }
        previous_end = Some(end);
        expected_base = expected_base.checked_add(length)?;
    }
    if expected_base != name_count {
        return None;
    }

    let mut previous_run = 0usize;
    for index in 0..page_dir.len() / NAME_PAGE_DIRECTORY_ENTRY_LEN {
        let run = usize::from(read_u16(
            page_dir,
            index.checked_mul(NAME_PAGE_DIRECTORY_ENTRY_LEN)?,
        )?);
        if run < previous_run || run > run_count {
            return None;
        }
        previous_run = run;
    }
    Some(())
}

/// The validated context map, model directory, model blobs and bitstream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameModels<'a> {
    /// Previous-byte to model-id map.
    pub context: [u8; 256],
    /// The canonical models, in id order.
    pub models: Vec<Model<'a>>,
    /// The global MSB-first bitstream.
    pub stream: &'a [u8],
}

impl<'a> NameModels<'a> {
    /// Parses the context map, model directory and model blobs.
    ///
    /// The models must be contiguous, each with a valid canonical codebook, and every context
    /// must select an existing model. Returns `None` otherwise or when the area is truncated.
    #[must_use]
    pub fn parse(section: &'a [u8], models_at: usize) -> Option<Self> {
        let context_map = section.get(models_at..models_at.checked_add(NAME_CONTEXT_MAP_LEN)?)?;
        let count_at = models_at.checked_add(NAME_CONTEXT_MAP_LEN)?;
        let model_count = usize::from(read_u16(section, count_at)?);
        if model_count == 0 || model_count > MAX_NAME_MODELS {
            return None;
        }
        let directory_at = models_at.checked_add(NAME_CONTEXT_MAP_LEN + NAME_MODEL_COUNT_LEN)?;
        let blobs_at =
            directory_at.checked_add(model_count.checked_mul(NAME_MODEL_DIRECTORY_ENTRY_LEN)?)?;
        if blobs_at > section.len() {
            return None;
        }

        let mut models = Vec::with_capacity(model_count);
        let mut expected = 0usize;
        for index in 0..model_count {
            let entry_at =
                directory_at.checked_add(index.checked_mul(NAME_MODEL_DIRECTORY_ENTRY_LEN)?)?;
            let relative = usize::from(read_u16(section, entry_at)?);
            if relative != expected {
                return None;
            }
            let at = blobs_at.checked_add(relative)?;
            let max_bits = *section.get(at)?;
            if max_bits == 0 || max_bits > MAX_CODE_BITS {
                return None;
            }
            let model_counts_at = at.checked_add(1)?;
            let symbols_at = model_counts_at.checked_add(usize::from(max_bits) * 2)?;
            let counts = section.get(model_counts_at..symbols_at)?;
            let mut wide = [0u16; MAX_CODE_BITS as usize + 1];
            let mut symbol_count = 0usize;
            for (slot, count) in wide
                .iter_mut()
                .take(usize::from(max_bits) + 1)
                .skip(1)
                .zip(counts.as_chunks::<2>().0)
            {
                let value = u16::from_le_bytes(*count);
                *slot = value;
                symbol_count += usize::from(value);
            }
            if symbol_count == 0 {
                return None;
            }
            let symbols = section.get(symbols_at..symbols_at.checked_add(symbol_count)?)?;
            models.push(Model::new(max_bits, &wide, symbols)?);
            expected = symbols_at
                .checked_add(symbol_count)?
                .checked_sub(blobs_at)?;
        }
        let stream_at = blobs_at.checked_add(expected)?;
        if stream_at > section.len() {
            return None;
        }
        let context: [u8; 256] = context_map.try_into().ok()?;
        if context
            .iter()
            .any(|&model| usize::from(model) >= model_count)
        {
            return None;
        }

        Some(Self {
            context,
            models,
            stream: section.get(stream_at..)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a name-section index. The model area is not part of the index.
    fn index_section(
        name_count: usize,
        block_shift: u8,
        runs: &[(u32, u16, u16)],
        page_dir: &[u16],
    ) -> Vec<u8> {
        let block_count = name_count.div_ceil(1usize << block_shift);
        let mut section = Vec::new();
        section.extend_from_slice(&u32::try_from(name_count).unwrap().to_le_bytes());
        section.extend_from_slice(&u32::try_from(block_count).unwrap().to_le_bytes());
        section.extend_from_slice(&u16::try_from(runs.len()).unwrap().to_le_bytes());
        section.extend_from_slice(&u16::try_from(NAME_PAGE_COUNT).unwrap().to_le_bytes());
        for &entry in page_dir {
            section.extend_from_slice(&entry.to_le_bytes());
        }
        for &(first, length, base) in runs {
            section.extend_from_slice(&first.to_le_bytes());
            section.extend_from_slice(&length.to_le_bytes());
            section.extend_from_slice(&base.to_le_bytes());
        }
        while !section.len().is_multiple_of(4) {
            section.push(0);
        }
        section.resize(section.len() + block_count * 4, 0);
        section
    }

    /// The page directory the encoder writes for `runs`.
    fn page_directory(runs: &[(u32, u16, u16)]) -> Vec<u16> {
        (0..NAME_PAGE_COUNT)
            .map(|page| {
                let base = u32::try_from(page).unwrap() << NAME_PAGE_SHIFT;
                let first = runs
                    .iter()
                    .position(|&(start, length, _)| start + u32::from(length) > base)
                    .unwrap_or(runs.len());
                u16::try_from(first).unwrap()
            })
            .collect()
    }

    #[test]
    fn name_index_records_are_validated() {
        let runs = [(0x41u32, 2u16, 0u16), (0x100, 1, 2)];
        let page_dir = page_directory(&runs);
        let valid = index_section(3, 1, &runs, &page_dir);
        assert!(NameIndex::parse(&valid, 1).is_some());

        // Runs must start after their predecessor.
        let duplicate = [(0x41, 1, 0), (0x41, 1, 1)];
        assert!(NameIndex::parse(&index_section(2, 1, &duplicate, &page_dir), 1).is_none());
        let out_of_order = [(0x100, 1, 2), (0x41, 2, 0)];
        assert!(NameIndex::parse(&index_section(3, 1, &out_of_order, &page_dir), 1).is_none());

        // A run must cover at least one code point and end inside the Unicode range.
        let empty_run = [(0x41, 0, 0), (0x100, 1, 0)];
        assert!(NameIndex::parse(&index_section(1, 1, &empty_run, &page_dir), 1).is_none());
        let past_end = [(0x10_FFFF, 2, 0)];
        assert!(
            NameIndex::parse(
                &index_section(1, 1, &past_end, &page_directory(&past_end)),
                1
            )
            .is_none()
        );

        // A run must not map past the last name id.
        let past_names = [(0x41, 1, 3)];
        assert!(
            NameIndex::parse(
                &index_section(3, 1, &past_names, &page_directory(&past_names)),
                1
            )
            .is_none()
        );

        // A corrupt base id must not point at another run's names.
        let wrong_base = [(0x41, 1, 1)];
        assert!(
            NameIndex::parse(
                &index_section(2, 1, &wrong_base, &page_directory(&wrong_base)),
                1
            )
            .is_none()
        );
        // Overlapping runs would shadow the later run's names.
        let overlapping = [(0x41, 2, 0), (0x42, 1, 2)];
        assert!(
            NameIndex::parse(
                &index_section(3, 1, &overlapping, &page_directory(&overlapping)),
                1
            )
            .is_none()
        );

        // The page directory must be non-decreasing and stay inside the run array.
        let mut decreasing = page_dir.clone();
        *decreasing.last_mut().unwrap() = 0;
        assert!(NameIndex::parse(&index_section(3, 1, &runs, &decreasing), 1).is_none());
        let mut too_far = page_dir;
        too_far[0] = 3;
        assert!(NameIndex::parse(&index_section(3, 1, &runs, &too_far), 1).is_none());
    }

    #[test]
    fn lookups_follow_the_page_directory_and_runs() {
        let runs = [(0x41u32, 2u16, 0u16), (0x100, 1, 2)];
        let page_dir = page_directory(&runs);
        let section = index_section(3, 1, &runs, &page_dir);
        let index = NameIndex::parse(&section, 1).expect("a valid index");
        assert_eq!(index.id_of(0x41), Some(0));
        assert_eq!(index.id_of(0x42), Some(1));
        assert_eq!(index.id_of(0x100), Some(2));
        assert_eq!(index.id_of(0x43), None);
        assert_eq!(index.id_of(0x11_0000), None);
    }

    #[test]
    fn model_area_requires_contiguous_valid_models() {
        let mut section = vec![0u8; NAME_CONTEXT_MAP_LEN];
        section.extend_from_slice(&1u16.to_le_bytes());
        section.extend_from_slice(&0u16.to_le_bytes());
        section.push(1);
        section.extend_from_slice(&1u16.to_le_bytes());
        section.push(b'A');
        let models = NameModels::parse(&section, 0).expect("one one-bit model");
        assert_eq!(models.models.len(), 1);
        assert_eq!(models.stream, b"");

        let mut truncated = section.clone();
        truncated.pop();
        assert!(NameModels::parse(&truncated, 0).is_none());

        let mut unknown_context = section;
        unknown_context[0] = 1;
        assert!(NameModels::parse(&unknown_context, 0).is_none());
    }
}
