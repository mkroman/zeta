//! The `.yt` search command and posted YouTube links.
//!
//! The plugin impl in the crate root dispatches both events here, so the command and the URL
//! handler share the link recognition and reply formatting.

use zeta_plugin::irc::{plain_reply, send_lookup_reply, send_reply};
use zeta_plugin::{Client, CommandEvent, UrlEvent, ZetaError};

use super::format::{search_error_reply, search_reply, video_error_reply};
use super::urls::is_youtube_url;
use super::{UrlKind, YouTube, parse_youtube_url};

/// Replies to `.yt` with the top video matching the query.
///
/// An invocation whose arguments are themselves a YouTube URL resolves it as a link instead of
/// searching for it as a query, so the two event kinds do not double up.
pub(crate) async fn search_video(
    plugin: &YouTube,
    client: &Client,
    command: &CommandEvent,
) -> Result<(), ZetaError> {
    let args = command.args().trim();

    // An invocation whose arguments are a YouTube URL resolves the video as a link instead
    // of searching for the URL as a query. Other URLs are searchable text.
    if is_youtube_url(args) {
        return Ok(());
    }

    send_lookup_reply(
        client,
        command.channel(),
        plugin.search(args),
        search_reply,
        |error| plain_reply(search_error_reply(&error)),
    )
    .await?;

    Ok(())
}

/// Replies to a posted YouTube video link with the video's details.
pub(crate) async fn handle_url(
    plugin: &YouTube,
    client: &Client,
    url: &UrlEvent,
) -> Result<(), ZetaError> {
    let (url, channel) = (url.url(), url.channel());

    let Some(UrlKind::Video(video_id) | UrlKind::Short(video_id)) = parse_youtube_url(url) else {
        return Ok(());
    };

    let video = match plugin.get_video(&video_id).await {
        Ok(video) => video,
        Err(error) => {
            send_reply(client, channel, plain_reply(video_error_reply(&error)))?;

            return Ok(());
        }
    };

    send_reply(client, channel, plugin.video_reply(&video))?;

    Ok(())
}
