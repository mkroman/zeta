//! The reply flow shared by the mirroring plugins.
//!
//! Mirroring itself is domain logic in the `zeta-mirror` crate; this module carries the
//! IRC-facing glue every mirroring plugin uses: replying with the mirrored file's link, in the
//! channel, through the client.

use std::sync::Arc;

use irc::client::Client;
use serde::{Deserialize, Serialize};
use tracing::warn;

use zeta_mirror::{MediaId, Mirror, MirrorHandle};
use zeta_plugin::Context;
use zeta_plugin::irc::{plain_reply, send_reply, send_reply_via};

/// The mirror configuration shared by the mirroring plugins.
///
/// Flattened into a plugin's `[plugins.<name>]` settings, so both keys stay at the top level of
/// the plugin's section. The env fallbacks and defaults follow the convention in
/// [`MirrorHandle::resolve`]: `<PLUGIN>_S3_PREFIX` defaults to the plugin name, and
/// `<PLUGIN>_PUBLIC_URL_BASE` defaults to `https://pub.rwx.im/<plugin>`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct MirrorSettings {
    /// The key prefix that mirrored media are uploaded under.
    ///
    /// Falls back to the `<PLUGIN>_S3_PREFIX` environment variable, and to the plugin name when
    /// neither is set.
    pub prefix: Option<String>,
    /// The base URL used when linking to mirrored media.
    ///
    /// Falls back to the `<PLUGIN>_PUBLIC_URL_BASE` environment variable when unset. Links are
    /// built by appending the media id as a URL fragment, so the base must point at a viewer
    /// page that resolves the fragment — not directly at the bucket.
    pub public_url_base: Option<String>,
}

impl MirrorSettings {
    /// Resolves the plugin's mirror handle, or [`None`] when no shared mirror is configured.
    ///
    /// A plugin with its own enable flag must check that flag before calling this.
    #[must_use]
    pub fn resolve(&self, ctx: &Context, plugin_name: &str) -> Option<MirrorHandle> {
        MirrorHandle::resolve(
            ctx.shared.get::<Mirror>(),
            plugin_name,
            self.prefix.as_deref(),
            self.public_url_base.as_deref(),
        )
    }
}

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
    id: &MediaId,
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
            let _ = send_reply_via(&sender, &channel, plain_reply(link));
        }
    };

    match mirror.ensure_mirrored(url, id, on_mirrored).await {
        Ok(Some(link)) => {
            let _ = send_reply(client, &channel, plain_reply(&link));
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
