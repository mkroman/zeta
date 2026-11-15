//! One `.uni` entry: a single character or a run naming an emoji sequence as a whole.

use std::borrow::Cow;

use zeta_unicode::{emoji_name, general_category, name_or_label, write_name};

use super::render::render_entry_into;

/// One entry of a `.uni` listing, kept cheap until the budget decides to render it.
pub(super) enum Entry<'a> {
    /// A single character, named by its character or emoji name.
    Char(char),
    /// A run of two or more characters naming an emoji sequence as a whole.
    Sequence {
        /// The input text that matched, used for the codepoint list.
        text: &'a str,
        /// The published sequence name, before upper-casing.
        name: Cow<'static, str>,
    },
}

impl Entry<'_> {
    /// Renders the entry into `out`, reusing `name` for the entry's upper-cased name.
    ///
    /// `out` is cleared first, so a listing renders every entry through the same buffers.
    pub(super) fn render_into(&self, out: &mut String, name: &mut String) {
        match self {
            Self::Char(c) => {
                character_name_into(*c, name);
                let category = general_category(*c).long_name();
                let mut encoded = [0u8; 4];
                render_entry_into(c.encode_utf8(&mut encoded), name, category, out);
            }
            Self::Sequence {
                text,
                name: sequence_name,
            } => {
                write_uppercase(name, sequence_name);
                let base = text
                    .chars()
                    .next()
                    .expect("a sequence has a base character");
                let category = general_category(base).long_name();
                render_entry_into(text, name, category, out);
            }
        }
    }
}

/// Writes the upper-cased name a character entry shows into `out`.
pub(super) fn character_name_into(c: char, out: &mut String) {
    out.clear();
    let mut encoded = [0u8; 4];
    let text = c.encode_utf8(&mut encoded);
    if let Some(name) = emoji_name(text) {
        write_uppercase(out, name.as_ref());
    } else if !write_name(c, out) {
        out.push_str(&name_or_label(c));
    }
}

/// Upper-cases `name` into `out`, borrowing the buffer rather than allocating a string.
fn write_uppercase(out: &mut String, name: &str) {
    out.clear();
    for c in name.chars() {
        if c.is_lowercase() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
    }
}
