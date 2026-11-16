//! Formats YouTube API data as IRC messages.
//!
//! The API HTML-escapes titles, channel names, and category names, so every value is unescaped
//! and stripped of IRC formatting before it is interpolated into a reply.

use serde::{Deserialize, Serialize};

use zeta_plugin::irc::{labeled_reply, plain_reply};
use zeta_utils::duration;
use zeta_utils::irc;
use zeta_utils::text;

use super::model::{Search, Video};
use super::{Error, YouTube};

/// The safe search filter applied to YouTube search requests.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SafeSearch {
    /// Do not filter search results.
    #[default]
    None,
    /// Filter out content that is explicitly flagged as mature.
    Moderate,
    /// Filter out most potentially mature content.
    Strict,
}

impl SafeSearch {
    /// Returns the value of the `safeSearch` API parameter.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Moderate => "moderate",
            Self::Strict => "strict",
        }
    }
}

impl YouTube {
    /// Formats the reply for `video`, resolving its category and view count.
    pub(super) fn video_reply(&self, video: &Video) -> String {
        let snippet = video.snippet.as_ref();
        let category_id = snippet.map(|s| s.category_id.as_str());

        // The map is refreshed by the task started on load; the read is
        // stale-tolerant, so an unavailable API keeps serving the last known
        // categories. Cloning the `Arc` lets the category title be borrowed
        // after the closure instead of copied.
        #[allow(clippy::redundant_closure_for_method_calls)] // a bare `Option::cloned` is ambiguous
        let categories = self.video_categories.with_value(|cache| cache.cloned());
        let category = categories
            .as_deref()
            .zip(category_id)
            .and_then(|(categories, id)| categories.get(id))
            .map_or("unknown category", |category| {
                category.snippet.title.as_str()
            });

        let view_count = video
            .statistics
            .as_ref()
            .and_then(|s| str::parse::<u64>(&s.view_count).ok())
            .unwrap_or(0);

        format_video_message(video, category, view_count)
    }
}

/// Decodes HTML entities in an API text field and unstyles it for an IRC reply.
///
/// YouTube HTML-escapes titles, channel names, and category names in both the search and
/// videos responses.
fn sanitize_html(text: &str) -> String {
    irc::unstyle_text(&htmlize::unescape(text)).into_owned()
}

/// Formats the reply for the top video in a `.yt` search.
pub(crate) fn search_reply(results: Vec<Search>) -> String {
    results
        .into_iter()
        .find_map(|result| {
            let id = result.id.video_id?;

            Some((result.snippet, id))
        })
        .map_or_else(
            || plain_reply("No results"),
            |(snippet, id)| {
                let title = sanitize_html(&snippet.title);

                labeled_reply(
                    "YouTube",
                    format_args!("{title} - https://www.youtube.com/watch?v={id}"),
                )
            },
        )
}

/// Formats a failed `.yt` search.
pub(crate) fn search_error_reply(error: &Error) -> String {
    const CONTEXT: &str = "could not search youtube";

    match error {
        Error::NoResults => format!("{CONTEXT}: {error}"),
        Error::Api(_) => CONTEXT.to_owned(),
    }
}

/// Formats a failed video lookup.
pub(crate) fn video_error_reply(error: &Error) -> String {
    const CONTEXT: &str = "could not look up the video";

    match error {
        Error::NoResults => format!("{CONTEXT}: {error}"),
        Error::Api(_) => CONTEXT.to_owned(),
    }
}

/// Formats the message describing `video`, using the resolved `category` and `view_count`.
///
/// Live and upcoming streams are described as live streams along with their category and
/// current number of concurrent viewers (falling back to the total view count when
/// unavailable), while regular videos include only their duration.
pub(crate) fn format_video_message(video: &Video, category: &str, view_count: u64) -> String {
    let snippet = video.snippet.as_ref();
    let title = snippet.map_or_else(|| "‽".to_string(), |s| sanitize_html(&s.title));
    let channel_name = snippet.map_or_else(
        || "unknown channel".to_string(),
        |s| sanitize_html(&s.channel_title),
    );
    let category = sanitize_html(category);
    let view_count_formatted = text::format_number(view_count);
    let view_noun = text::pluralize_noun(view_count, "view");

    let is_live_stream =
        snippet.is_some_and(|s| matches!(s.live_broadcast_content.as_str(), "live" | "upcoming"));

    if is_live_stream {
        let article = text::indefinite_article(&category);

        if let Some(viewers) = video
            .live_streaming_details
            .as_ref()
            .and_then(|details| details.concurrent_viewers.as_deref())
            .and_then(|viewers| viewers.parse::<u64>().ok())
        {
            return plain_reply(format_args!(
                "“\x0f{title}\x0310” is {article}\x0f {category}\x0310 live stream by\x0f \
                 {channel_name}\x0310 with\x0f {}\x0310 {}",
                text::format_number(viewers),
                text::pluralize_noun(viewers, "viewer"),
            ));
        }

        return plain_reply(format_args!(
            "“\x0f{title}\x0310” is {article}\x0f {category}\x0310 live stream by\x0f \
             {channel_name}\x0310 with\x0f {view_count_formatted}\x0310 {view_noun}",
        ));
    }

    let duration = video
        .content_details
        .as_ref()
        .and_then(|details| duration::parse_iso8601_duration(&details.duration))
        .map_or_else(|| "unknown duration".to_string(), duration::format_duration);
    let article = text::indefinite_article(&duration);

    plain_reply(format_args!(
        "“\x0f{title}\x0310” is {article}\x0f {duration}\x0310 video by\x0f \
         {channel_name}\x0310 with\x0f {view_count_formatted}\x0310 {view_noun}",
    ))
}
