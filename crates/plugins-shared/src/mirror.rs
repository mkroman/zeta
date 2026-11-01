//! The reply flow shared by the mirroring plugins.
//!
//! Mirroring itself is domain logic in the `zeta-mirror` crate; this module carries the
//! IRC-facing glue every mirroring plugin uses: replying with the mirrored file's link, in the
//! channel, through the client.

use std::sync::Arc;

use irc::client::Client;
use tracing::warn;

use zeta_mirror::MirrorHandle;
use zeta_plugin::prelude::*;

/// Mirrors the media at `url` and replies with the mirrored file's link, when the plugin has a
/// mirror configured.
///
/// Without a configured mirror this is a no-op, leaving the plugin to its non-mirroring
/// behavior. If the media has already been mirrored, the existing link is sent immediately;
/// otherwise the download and upload happen in a background task that replies with the link once
/// it is ready. A failed mirror check is logged and leaves the channel without a reply. The
/// channel is shared as an `Arc<str>` because the reply callback outlives this call.
pub async fn ensure_mirrored_if_configured(
    mirror: Option<&MirrorHandle>,
    url: &str,
    id: &str,
    channel: &str,
    client: &Client,
) {
    let Some(mirror) = mirror else {
        return;
    };

    let sender = client.sender();
    let channel: Arc<str> = Arc::from(channel);

    let on_mirrored = {
        let channel = Arc::clone(&channel);
        move |link: String| {
            let _ = sender.send_privmsg(&channel, plain_reply(link));
        }
    };

    match mirror.ensure_mirrored(url, id, on_mirrored).await {
        Ok(Some(link)) => {
            let _ = client.send_privmsg(&channel, plain_reply(&link));
        }
        Ok(None) => {}
        Err(error) => {
            warn!(
                media.id = %id,
                exception.message = %error,
                "could not check if the media is already mirrored"
            );
        }
    }
}
