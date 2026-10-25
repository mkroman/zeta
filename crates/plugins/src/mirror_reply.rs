//! The reply flow shared by the mirroring plugins.
//!
//! Mirroring itself is domain logic in the `zeta-mirror` crate; this module carries the
//! IRC-facing glue every mirroring plugin uses: replying with the mirrored file's link, in the
//! channel, through the client.

use irc::client::Client;
use tracing::error;

use crate::prelude::*;
use zeta_mirror::MirrorHandle;

/// Starts the shared download manager when the plugin has a mirror configured.
///
/// Plugins call this from their `loaded` hook; without a configured mirror it is a no-op.
pub fn start_downloads(mirror: Option<&MirrorHandle>) {
    if let Some(mirror) = mirror {
        mirror.start_downloads();
    }
}

/// Mirrors the media at `url` and replies with the mirrored file's link, when the plugin has a
/// mirror configured.
///
/// Without a configured mirror this is a no-op, leaving the plugin to its non-mirroring
/// behavior.
pub async fn ensure_mirrored_if_configured(
    mirror: Option<&MirrorHandle>,
    url: &str,
    id: &str,
    channel: &str,
    client: &Client,
) {
    if let Some(mirror) = mirror {
        ensure_mirrored_and_reply(mirror, url, id, channel, client).await;
    }
}

/// Mirrors the media at `url` with the given `id`, replying with the mirrored file's link to
/// `channel` through `client`.
///
/// If the media has already been mirrored, the existing link is sent immediately; otherwise the
/// download and upload happen in a background task that replies with the link once it is ready.
/// A failed mirror check is logged and leaves the channel without a reply.
pub async fn ensure_mirrored_and_reply(
    mirror: &MirrorHandle,
    url: &str,
    id: &str,
    channel: &str,
    client: &Client,
) {
    let sender = client.sender();
    let channel = channel.to_string();

    let on_mirrored = {
        let channel = channel.clone();
        move |link: String| {
            let _ = sender.send_privmsg(&channel, notice(link));
        }
    };

    match mirror.ensure_mirrored(url, id, on_mirrored).await {
        Ok(Some(link)) => {
            let _ = client.send_privmsg(&channel, notice(&link));
        }
        Ok(None) => {}
        Err(error) => {
            error!(%id, %error, "could not check if the media is already mirrored");
        }
    }
}
