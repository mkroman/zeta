//! Media and story orchestration.
//!
//! A link is classified through [`parse_instagram_url`]: media posts and stories are summarized
//! and queued for mirroring through [`Instagram::post_summary`], while share links are resolved
//! through their redirect first.

use std::future::Future;

use tracing::{debug, warn};
use url::Url;

use zeta_mirror::MediaId;
use zeta_plugin::Client;
use zeta_plugin::irc::{plain_reply, send_reply};
use zeta_url::RedactedUrl;

use crate::format::format_summary;
use crate::meta::MediaDetails;
use crate::urls::{InstagramLink, MediaKind, media_url, parse_instagram_url, story_url};
use crate::{Error, Instagram};

impl Instagram {
    pub(super) async fn process_url(
        &self,
        url: &Url,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        match parse_instagram_url(url) {
            Some(InstagramLink::Media { kind, id }) => {
                debug!(instagram.media.id = %id, "processing media");

                self.process_media(&kind, &id, channel, client).await?;
            }
            Some(InstagramLink::Story { username, id }) => {
                debug!(instagram.media.id = %id, instagram.username = %username, "processing story");

                self.process_story(&username, &id, channel, client).await?;
            }
            Some(InstagramLink::Shortened(url)) => {
                debug!(url.full = %RedactedUrl::from_url(&url), "resolving share link");

                self.throttle().await;

                let resolved_url = self.resolve_url(&url).await?;

                if let Some(InstagramLink::Media { kind, id }) = parse_instagram_url(&resolved_url)
                {
                    self.process_media(&kind, &id, channel, client).await?;
                }
            }
            // Profile links are only claimed so that they are not announced by the titles plugin.
            Some(InstagramLink::Profile(_)) | None => {}
        }

        Ok(())
    }

    /// Posts a summary of the media with the given kind and id, and queues it for mirroring.
    ///
    /// The details are fetched and cached through [`Self::post_summary`].
    async fn process_media(
        &self,
        kind: &MediaKind,
        id: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let url = media_url(*kind, id);

        self.post_summary(
            id,
            &url,
            kind.label(),
            || async { self.fetch_media_details(id, &url, *kind).await },
            channel,
            client,
        )
        .await
    }

    /// Posts a summary of the story published by `username` with the given id, and queues it
    /// for mirroring.
    ///
    /// Stories have no anonymously accessible API, so only their page is consulted, which
    /// generally carries details only with a session cookie configured.
    async fn process_story(
        &self,
        username: &str,
        id: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let url = story_url(username, id);

        self.post_summary(
            id,
            &url,
            "story",
            || async {
                self.fetch_page_details(&url, self.session_cookie.as_deref())
                    .await
            },
            channel,
            client,
        )
        .await
    }

    /// Posts the summary for the cached details of a media or story, and queues the video for
    /// mirroring.
    ///
    /// The details for `id` are fetched through `refresh` when the cache is stale; a failed
    /// refresh is logged and skips the summary only.
    async fn post_summary<E, F, Fut>(
        &self,
        id: &str,
        url: &str,
        label: &str,
        refresh: F,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Option<MediaDetails>, E>> + Send,
        E: std::fmt::Display,
    {
        let details = match self.details.get_or_refresh(id.to_string(), refresh).await {
            Ok(details) => details,
            Err(error) => {
                debug!(
                    instagram.media.id = %id,
                    instagram.resource = %label,
                    exception.message = %error,
                    "could not fetch details"
                );

                None
            }
        };

        if let Some(summary) = details
            .as_ref()
            .and_then(|details| format_summary(details, label, self.settings.caption_length))
            && let Err(error) = send_reply(client, channel, plain_reply(&summary))
        {
            warn!(exception.message = %error, "could not send the instagram summary");
        }

        let Some(mirror) = self.mirror.as_ref() else {
            return Ok(());
        };

        let Ok(media_id) = MediaId::parse(id) else {
            warn!(media.id = %id, "ignoring mirror request with an unsafe id");

            return Ok(());
        };

        plugins_shared::mirror::ensure_mirrored_if_configured(
            Some(mirror),
            url,
            &media_id,
            channel,
            client,
        )
        .await;

        Ok(())
    }
}

/// Returns the URL of the media's page on the configured metadata proxy.
///
/// The proxy serves the OpenGraph metadata of Instagram media, so the media path is the canonical
/// one.
#[must_use]
pub fn proxy_media_url(proxy: &str, kind: MediaKind, id: &str) -> String {
    format!("{}/{}/{}", proxy.trim_end_matches('/'), kind.segment(), id)
}
