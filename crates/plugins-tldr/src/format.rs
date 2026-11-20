//! Rendering a Kagi summary as the `> TLDR:` reply lines.
//!
//! The endpoint's markdown body is plain text with `-` bullets and a leading `Title:` line.
//! The reply drops the title (the page is already linked or named in the channel), renders
//! bullets as `•`, strips the markdown emphasis Kagi sprinkles in, and either truncates the
//! result into one budgeted line or wraps the whole summary across several.

use zeta_plugin::irc::{
    LABELED_PREFIX_BYTES, MAX_LISTING_LENGTH, PLAIN_REPLY_PREFIX, labeled_reply,
    labeled_reply_prefix, unstyle_text,
};
use zeta_utils::text::TruncatableExt;

use crate::preferences::Delivery;

/// The label every reply line carries or continues.
pub(crate) const LABEL: &str = "TLDR";

/// The default maximum length of a truncated summary, in bytes of summary text.
///
/// The line budget minus the labeled `> TLDR: ` framing and room for the ellipsis, so a
/// truncated reply always fits a single IRC line.
pub(crate) const DEFAULT_MAX_SUMMARY_LENGTH: usize =
    MAX_LISTING_LENGTH - LABELED_PREFIX_BYTES - LABEL.len() - 4;

/// The budget of a continuation line: the line budget minus the marker-only prefix and room
/// for the ellipsis.
const CONTINUATION_BUDGET: usize = MAX_LISTING_LENGTH - PLAIN_REPLY_PREFIX.len() - 4;

/// The budget of the first line: the line budget minus the labeled prefix and room for the
/// ellipsis.
const FIRST_BUDGET: usize = MAX_LISTING_LENGTH - LABELED_PREFIX_BYTES - LABEL.len() - 4;

/// Renders the markdown body as one line of IRC text.
///
/// The leading `Title:` line is dropped, `- ` bullets become `•` separators (with a leading
/// `•` when the body is a bullet list at all), markdown emphasis is stripped, and IRC styling
/// from the summary text is removed. Empty lines vanish; the result is single-spaced.
pub(crate) fn body(markdown: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut bulleted = false;

    for line in markdown.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // The body opens with `Title: <page title>` on every response seen so far.
        if parts.is_empty() && line.starts_with("Title:") {
            continue;
        }

        match line.strip_prefix("- ") {
            Some(item) => {
                bulleted = true;
                parts.push(item.trim());
            }
            None => parts.push(line),
        }
    }

    let joined = parts.join(if bulleted { " • " } else { " " });
    let rendered = if bulleted {
        format!("• {joined}")
    } else {
        joined
    };

    // Markdown emphasis and inline code carry no meaning in IRC.
    let rendered = rendered.replace("**", "").replace('`', "");

    unstyle_text(&rendered).into_owned()
}

/// Renders the reply lines for `markdown` at `delivery`, truncating to `max` bytes when the
/// delivery is truncated.
///
/// Every returned line is within the IRC line budget; a truncated reply is one line with the
/// labeled prefix, a full reply starts with the labeled line and continues with marker-only
/// lines.
pub(crate) fn reply_lines(markdown: &str, delivery: Delivery, max: usize) -> Vec<String> {
    let text = body(markdown);
    if text.is_empty() {
        return Vec::new();
    }

    match delivery {
        Delivery::Truncated => {
            // The budget is bytes — the IRC line limit is — so the cut is byte-based and lands
            // on a character boundary with the ellipsis included.
            let truncated = text.truncate_bytes_within(max, "…");
            vec![labeled_reply(LABEL, truncated.into_owned())]
        }
        Delivery::Full => full_lines(&text),
    }
}

/// Wraps `text` across the reply lines of a full reply.
///
/// The first line carries the `TLDR` label, the rest the plain marker; the budgets leave room
/// for those prefixes and the ellipsis. A word longer than a line is cut mid-word, marked with
/// the ellipsis like any other cut.
fn full_lines(text: &str) -> Vec<String> {
    let mut wrapped = wrap(text, FIRST_BUDGET, CONTINUATION_BUDGET);
    if wrapped.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::with_capacity(wrapped.len());
    let first = wrapped.remove(0);
    lines.push(format!("{}{first}", labeled_reply_prefix(LABEL)));

    for line in wrapped {
        lines.push(format!("{PLAIN_REPLY_PREFIX}{line}"));
    }

    lines
}

/// Greedy word wrap of `text` into lines of at most `first_budget` (the first line) and
/// `rest_budget` (the rest) bytes, on char boundaries.
///
/// A word longer than the budget is cut at the budget with an ellipsis, and the remainder
/// continues on the next line, so no line ever exceeds its budget.
fn wrap(text: &str, first_budget: usize, rest_budget: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut budget = first_budget;

    for word in text.split(' ').filter(|word| !word.is_empty()) {
        let mut remaining = word;

        loop {
            let separator = usize::from(!line.is_empty());
            let room = budget.saturating_sub(line.len() + separator);

            if remaining.len() <= room {
                if separator == 1 {
                    line.push(' ');
                }
                line.push_str(remaining);

                break;
            }

            if line.is_empty() {
                // The word alone exceeds the line: cut it at the budget with an ellipsis and
                // carry the remainder onto the next line. The ellipsis is budgeted, so the
                // line stays within its budget.
                let cut = floor_char_boundary(remaining, room.saturating_sub("…".len()));
                let (head, tail) = remaining.split_at(cut);
                lines.push(format!("{head}…"));
                remaining = tail;
                budget = rest_budget;

                continue;
            }

            // The line is full: close it, and retry the word (or remainder) on a fresh one.
            lines.push(std::mem::take(&mut line));
            budget = rest_budget;
        }
    }

    if !line.is_empty() {
        lines.push(line);
    }

    lines
}

/// The largest index of `text` at or before `index` that is a char boundary.
const fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }

    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_plugin::irc::{BOLD, COLOR, REPLY_MARKER};
    use zeta_test_support::assert_empty;

    /// The markdown of a prose summary: a title line and one paragraph.
    const PROSE_MARKDOWN: &str = "Title: A page | Site\n\nFirst sentence. Second sentence.";

    /// The markdown of a takeaway: a title line and bullets.
    const TAKEAWAY_MARKDOWN: &str =
        "Title: A page | Site\n\n- one point\n- another point\n- a third point";

    /// The exact labeled prefix of a `TLDR` reply, per the reply format.
    fn labeled(text: &str) -> String {
        format!("{REPLY_MARKER}\x0f{BOLD} {LABEL}:{BOLD}{COLOR} {text}")
    }

    #[test]
    fn the_title_line_is_dropped() {
        let rendered = body(PROSE_MARKDOWN);

        assert_eq!(rendered, "First sentence. Second sentence.");
        assert!(
            !rendered.contains("Title:"),
            "the title never reaches the channel"
        );
    }

    #[test]
    fn bullets_render_with_leading_dots() {
        let rendered = body(TAKEAWAY_MARKDOWN);

        assert_eq!(rendered, "• one point • another point • a third point");
    }

    #[test]
    fn emphasis_and_styling_are_stripped() {
        let rendered = body("Title: T\n\n**bold** and `code` and \x02styled\x0f");

        assert_eq!(rendered, "bold and code and styled");
    }

    #[test]
    fn an_empty_body_renders_nothing() {
        assert_empty!(body("Title: only a title\n\n"));
        assert_empty!(reply_lines("Title: only a title\n\n", Delivery::Full, 380));
    }

    #[test]
    fn a_truncated_reply_is_one_budgeted_line_with_the_label() {
        let long = format!("Title: T\n\n{}", "word ".repeat(200));
        let lines = reply_lines(&long, Delivery::Truncated, 380);

        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with('…'), "the cut is visible: {}", lines[0]);
        assert!(
            lines[0].len() <= MAX_LISTING_LENGTH,
            "the line fits the budget"
        );
        assert!(
            lines[0].starts_with(&labeled("")),
            "the reply opens with the labeled prefix"
        );
    }

    #[test]
    fn a_truncated_reply_is_byte_budgeted_for_multibyte_text() {
        // Danish news summaries are the norm for this bot: 400 two-byte words would blow the
        // 512-byte IRC line if the cut counted characters instead of bytes.
        let danish = format!("Title: T\n\n{}", "æ ".repeat(400));
        let lines = reply_lines(&danish, Delivery::Truncated, 380);

        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].len() <= MAX_LISTING_LENGTH,
            "the multibyte line fits the byte budget: {} bytes",
            lines[0].len()
        );
        assert!(lines[0].ends_with('…'), "the cut is visible");

        // CJK is the worst case: three bytes per character.
        let cjk = format!("Title: T\n\n{}", "語 ".repeat(400));
        let lines = reply_lines(&cjk, Delivery::Truncated, 380);

        assert!(
            lines[0].len() <= MAX_LISTING_LENGTH,
            "the CJK line fits the byte budget: {} bytes",
            lines[0].len()
        );
    }

    #[test]
    fn a_short_truncated_reply_is_not_cut() {
        let lines = reply_lines(PROSE_MARKDOWN, Delivery::Truncated, 380);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], labeled("First sentence. Second sentence."));
    }

    #[test]
    fn a_full_reply_wraps_within_the_budgets() {
        let long = format!("Title: T\n\n{}", "word ".repeat(300));
        let lines = reply_lines(&long, Delivery::Full, 380);

        assert!(
            lines.len() > 3,
            "a long summary wraps: {} lines",
            lines.len()
        );
        assert!(
            lines[0].starts_with(REPLY_MARKER),
            "the first line is labeled"
        );
        for line in &lines[1..] {
            assert!(
                line.starts_with(PLAIN_REPLY_PREFIX),
                "continuations carry the marker: {line}"
            );
        }
        for line in &lines {
            assert!(
                line.len() <= MAX_LISTING_LENGTH,
                "every line fits: {}",
                line.len()
            );
        }
        assert!(
            lines.concat().contains("word word word"),
            "no content is dropped when wrapping"
        );
    }

    #[test]
    fn a_takeaway_full_reply_keeps_every_bullet() {
        let lines = reply_lines(TAKEAWAY_MARKDOWN, Delivery::Full, 380);

        assert_eq!(lines.len(), 1, "a short summary fits one line");
        assert!(lines[0].contains("• one point • another point • a third point"));
    }

    #[test]
    fn a_word_longer_than_a_line_is_cut_with_an_ellipsis() {
        let monster = "x".repeat(1000);
        let long = format!("Title: T\n\n{monster}");
        let lines = reply_lines(&long, Delivery::Full, 380);

        assert!(lines.len() > 1);
        for line in &lines {
            assert!(
                line.len() <= MAX_LISTING_LENGTH,
                "every line fits: {}",
                line.len()
            );
        }

        // The cut branch keeps its own contract: every wrapped chunk, ellipsis included, fits
        // the budget it was wrapped against — with the reply prefixes the lines stay within the
        // line budget exactly, not within some slack around it.
        let wrapped = wrap(&monster, 100, 100);

        assert!(wrapped.len() > 5, "a monster word wraps repeatedly");
        for line in &wrapped {
            assert!(
                line.len() <= 100,
                "the cut chunk fits its budget: {}",
                line.len()
            );
        }
    }

    #[test]
    fn wrapping_is_char_boundary_safe() {
        // Each `æ` is two bytes, so most byte-level cuts would land mid-character; the wrap
        // must step back to a boundary instead of panicking in `split_at`.
        let text = "æ ".repeat(400);
        let lines = reply_lines(&format!("Title: T\n\n{text}"), Delivery::Full, 380);

        assert!(lines.len() > 1);
        for line in &lines {
            assert!(
                line.len() <= MAX_LISTING_LENGTH,
                "every line fits: {}",
                line.len()
            );
        }
        assert!(
            lines.concat().contains("æ æ æ"),
            "the wrapped text keeps its content: {}",
            lines.concat()
        );
    }
}
