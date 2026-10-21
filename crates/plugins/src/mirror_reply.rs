//! The reply flow shared by the mirroring plugins.
//!
//! Mirroring itself is domain logic in the `zeta-mirror` crate; this module carries the
//! IRC-facing glue every mirroring plugin uses: replying with the mirrored file's link, in the
//! channel, through the client.

use irc::client::Client as IrcClient;
use tracing::error;

use crate::prelude::*;
use zeta_mirror::MirrorHandle;

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
    client: &IrcClient,
) {
    let sender = client.sender();
    let channel = channel.to_string();

    let on_mirrored = {
        let channel = channel.clone();
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
            error!(%id, %error, "could not check if the media is already mirrored");
        }
    }
}
