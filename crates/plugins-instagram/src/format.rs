//! Formatting of media summaries.
//!
//! The summary is a single IRC line, composed by hand from control codes: untrusted caption and
//! author text is stripped of formatting first, and the caption is truncated to the configured
//! length.

use std::fmt::Write;

use zeta_utils::irc;
use zeta_utils::text::TruncatableExt;

use crate::meta::MediaDetails;

/// Formats the media details as a human-readable summary of the media.
pub fn format_summary(details: &MediaDetails, kind: &str, caption_length: usize) -> Option<String> {
    let mut buf = String::new();

    if let Some(caption) = details
        .caption
        .as_deref()
        .map(irc::unstyle_text)
        .filter(|caption| !caption.is_empty())
    {
        let truncated = caption.truncate_with_suffix(caption_length, "…");
        let _ = write!(buf, "“\x0f{}\x0310” ", truncated.trim());
    }

    if let Some(author) = details
        .author
        .as_deref()
        .map(irc::unstyle_text)
        .filter(|author| !author.is_empty())
    {
        if buf.is_empty() {
            let _ = write!(buf, "Instagram {kind} by\x0f {author}");
        } else {
            let _ = write!(buf, "is an Instagram {kind} by\x0f {author}");
        }
    } else if !buf.is_empty() {
        let _ = write!(buf, "is an Instagram {kind}");
    }

    (!buf.is_empty()).then_some(buf)
}
