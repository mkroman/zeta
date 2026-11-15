//! Lazy grouping of the `.uni` input into character and emoji-sequence entries.

use std::borrow::Cow;
use std::iter::Peekable;
use std::str::CharIndices;

use zeta_unicode::{MAX_EMOJI_KEY_BYTES, emoji_name, may_start_emoji_sequence};

use super::entry::Entry;

/// Longest run of characters `.uni` probes for an emoji sequence.
///
/// A stored emoji key is at most [`MAX_EMOJI_KEY_BYTES`] bytes, hence at most that many
/// characters. Only stripped `U+FE0F` padding can make a matching input run longer, so the cap
/// bounds the probe work: a longer padded run is split into a matched prefix and the remaining
/// characters rather than hiding the rest of the input.
pub(super) const MAX_GROUPING_CHARS: usize = MAX_EMOJI_KEY_BYTES;

/// The character entries of an input, grouped lazily as [`grouped_entries`] describes.
pub(super) struct GroupedEntries<'a> {
    /// The input text every entry borrows from.
    args: &'a str,
    /// The characters that have not been grouped yet.
    chars: Peekable<CharIndices<'a>>,
}

impl<'a> GroupedEntries<'a> {
    /// Starts grouping `args` from its first character.
    pub(super) fn new(args: &'a str) -> Self {
        Self {
            args,
            chars: args.char_indices().peekable(),
        }
    }
}

impl<'a> Iterator for GroupedEntries<'a> {
    type Item = Entry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let (start, c) = self.chars.next()?;
        // Plain text cannot start a stored key, so the admission check rejects it before
        // probing every candidate end.
        let Some((end, name)) = longest_sequence(self.args, start, &self.chars) else {
            return Some(Entry::Char(c));
        };

        while self.chars.peek().is_some_and(|&(offset, _)| offset < end) {
            self.chars.next();
        }

        Some(Entry::Sequence {
            text: &self.args[start..end],
            name,
        })
    }
}

/// Splits `args` into character entries, grouping each run that names an emoji sequence of at
/// least two code points into a single entry.
#[cfg(test)]
pub(super) fn grouped_entries(args: &str) -> Vec<Entry<'_>> {
    GroupedEntries::new(args).collect()
}

/// The longest run starting at `start` that names an emoji sequence, with the byte offset just
/// past it.
///
/// A run is at most [`MAX_GROUPING_CHARS`] characters long, so the probe reads a bounded window
/// into a fixed-size stack buffer instead of materializing the whole input.
fn longest_sequence(
    args: &str,
    start: usize,
    rest: &Peekable<CharIndices<'_>>,
) -> Option<(usize, Cow<'static, str>)> {
    if !may_start_emoji_sequence(&args[start..]) {
        return None;
    }

    // `ends[length]` is the byte offset just past a run of `length` characters; the slot after
    // the last filled one is the end of the longest run the cap allows.
    let mut ends = [args.len(); MAX_GROUPING_CHARS + 1];
    let mut lookahead = rest.clone();
    let mut length = 1usize;

    while length < MAX_GROUPING_CHARS {
        let Some((offset, _)) = lookahead.next() else {
            break;
        };
        ends[length] = offset;
        length += 1;
    }
    ends[length] = lookahead.peek().map_or(args.len(), |&(offset, _)| offset);

    (2..=length)
        .rev()
        .find_map(|end| emoji_name(&args[start..ends[end]]).map(|name| (ends[end], name)))
}
