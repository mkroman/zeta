//! Recognizes YouTube links in command arguments.
//!
//! Posted links are classified by the shared URL parser; this module only answers whether a
//! piece of text is one, which decides between resolving and searching in `.yt`.

use url::Url;

use plugins_shared::urls::youtube::parse_youtube_url;

/// Returns whether `text` is a YouTube URL.
///
/// The `.yt` command treats a YouTube URL as the link it stands for, while other text is an
/// ordinary search query.
#[must_use]
pub(crate) fn is_youtube_url(text: &str) -> bool {
    Url::parse(text.trim()).is_ok_and(|url| parse_youtube_url(&url).is_some())
}
