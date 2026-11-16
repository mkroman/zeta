//! Canonical URL construction for Instagram media and stories.

use super::classify::MediaKind;

/// Returns the canonical URL for the given media.
///
/// The canonical URL is built on the standard host; the mirror and metadata requests always use
/// it, no matter which host the link was posted with.
#[must_use]
pub(crate) fn media_url(kind: MediaKind, id: &str) -> String {
    format!("https://www.instagram.com/{}/{}", kind.segment(), id)
}

/// Returns the canonical URL for the given story.
#[must_use]
pub(crate) fn story_url(username: &str, id: &str) -> String {
    format!("https://www.instagram.com/stories/{username}/{id}")
}
