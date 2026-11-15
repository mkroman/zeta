//! Fetching media details and throttling requests to Instagram.
//!
//! The OpenGraph metadata of the canonical page is tried first, then the anonymous GraphQL API,
//! then the optionally configured metadata proxy. Requests are throttled through
//! [`Instagram::throttle`], so bursts of links do not trip Instagram's rate limiting.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::debug;
use url::Url;

use zeta_http as http;
use zeta_http::emulated::ResponseError;
use zeta_plugin::Chain;
use zeta_url::RedactedUrl;

use crate::graphql;
use crate::media::proxy_media_url;
use crate::meta::{self, MediaDetails};
use crate::urls::MediaKind;
use crate::{Error, Instagram};

/// The minimum interval between requests to Instagram, so that bursts of links do not trip its
/// rate limiting.
const REQUEST_INTERVAL: Duration = Duration::from_secs(2);

impl Instagram {
    /// Fetches the details of the media with the given shortcode.
    ///
    /// The OpenGraph metadata of the canonical media page is tried first, then the anonymous
    /// GraphQL API, then the configured metadata proxy. A page request that errors is retried
    /// once, so a transient failure does not cost the summary.
    ///
    /// Returns `Ok(None)` when no source carried details — a negative result the caller caches.
    /// An error is returned when a source failed with a request error instead of answering
    /// without details: the outcome is unknown then, and the caller does not cache it, so a
    /// later mention of the same link tries again.
    pub(super) async fn fetch_media_details(
        &self,
        id: &str,
        canonical: &str,
        kind: MediaKind,
    ) -> Result<Option<MediaDetails>, Error> {
        let mut details = None;
        let mut failed = false;

        self.try_source(
            id,
            "media page",
            self.fetch_page_details(canonical, self.session_cookie.as_deref()),
            &mut details,
            &mut failed,
        )
        .await;

        if details.is_none() {
            debug!(instagram.media.id = %id, "falling back to the graphql api");

            self.try_source(
                id,
                "graphql api",
                self.fetch_graphql_details(id),
                &mut details,
                &mut failed,
            )
            .await;
        }

        if details.is_none()
            && let Some(proxy) = &self.metadata_proxy
        {
            let url = proxy_media_url(proxy, kind, id);
            debug!(url.full = %RedactedUrl::from_unparsed(&url), "falling back to the metadata proxy");

            self.try_source(
                id,
                "metadata proxy",
                self.fetch_page_details(&url, None),
                &mut details,
                &mut failed,
            )
            .await;
        }

        if details.is_none() && failed {
            return Err(Error::SourcesFailed);
        }

        Ok(details)
    }

    /// Awaits one source of details, storing the result in `details` and logging a failure.
    ///
    /// A failed source sets `failed`, which turns into an error when no source ends up carrying
    /// details: the outcome is unknown then, and the caller does not cache it, so a later
    /// mention of the same link tries again.
    async fn try_source<E, Fut>(
        &self,
        id: &str,
        context: &str,
        source: Fut,
        details: &mut Option<MediaDetails>,
        failed: &mut bool,
    ) where
        Fut: Future<Output = Result<Option<MediaDetails>, E>>,
        E: std::fmt::Display,
    {
        match source.await {
            Ok(source_details) => *details = source_details,
            Err(error) => {
                debug!(
                    instagram.media.id = %id,
                    instagram.resource = %context,
                    exception.message = %error,
                    "instagram request failed"
                );

                *failed = true;
            }
        }
    }

    /// Fetches the media details through the anonymous GraphQL API.
    ///
    /// The front-page session tokens the request needs are cached, so a run of gated media does
    /// not each fetch the front page. Returns `Ok(None)` when the media is not anonymously
    /// accessible, and an error when the requests fail.
    async fn fetch_graphql_details(
        &self,
        id: &str,
    ) -> Result<Option<MediaDetails>, graphql::Error> {
        self.throttle().await;

        let session = self
            .session
            .get_or_refresh(|| async {
                graphql::fetch_session(&self.client, self.session_cookie.as_deref())
                    .await
                    .map(Arc::new)
            })
            .await?;

        match graphql::fetch_media_details(
            &self.client,
            id,
            &session,
            self.session_cookie.as_deref(),
        )
        .await
        {
            Ok(details) => Ok(Some(details)),
            Err(error @ graphql::Error::Gated) => {
                debug!(
                    instagram.media.id = %id,
                    exception.message = %error,
                    "the graphql api carried no details"
                );

                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Fetches a page and extracts its OpenGraph details.
    ///
    /// Returns `Ok(None)` when the page carried no details, and an error when the request
    /// failed.
    pub(super) async fn fetch_page_details(
        &self,
        url: &str,
        session_cookie: Option<&str>,
    ) -> Result<Option<MediaDetails>, meta::Error> {
        let details = MediaDetails::from_og(&self.fetch_page_metadata(url, session_cookie).await?);

        if details.is_none() {
            debug!(url.full = %RedactedUrl::from_unparsed(url), "the page carried no media details");
        }

        Ok(details)
    }

    /// Fetches a page, retrying the request once after a pause.
    ///
    /// Client errors are not retried: a dead link or a rejected request would fail the retry all
    /// the same, and the pause would only delay the fallbacks that follow.
    async fn fetch_page_metadata(
        &self,
        url: &str,
        session_cookie: Option<&str>,
    ) -> Result<meta::PageMetadata, meta::Error> {
        self.throttle().await;

        let error = match meta::fetch(&self.client, url, session_cookie).await {
            Ok(metadata) => return Ok(metadata),
            Err(error) => error,
        };

        debug!(
            url.full = %RedactedUrl::from_unparsed(url),
            exception.message = %Chain(&error),
            "could not fetch page metadata"
        );

        if let ResponseError::Status(status) = &error
            && status.is_client_error()
        {
            return Err(error);
        }

        // A failed request is retried once after a pause, so a transient failure does not cost
        // the summary.
        self.throttle().await;

        meta::fetch(&self.client, url, session_cookie)
            .await
            .inspect_err(|error| {
                debug!(
                    url.full = %RedactedUrl::from_unparsed(url),
                    exception.message = %Chain(error),
                    "the page request failed again"
                );
            })
    }

    /// Requests the given share link and returns the URL it resolves to.
    pub(super) async fn resolve_url(&self, url: &Url) -> Result<Url, Error> {
        debug!(url.full = %RedactedUrl::from_url(url), "resolving share url");

        let response = http::emulated::send(self.client.get(url.as_str())).await?;

        // The final URI, after the followed redirects, identifies the shared resource.
        let location = response.uri().to_string();
        let url = Url::parse(&location).map_err(|_| Error::InvalidRedirect)?;

        debug!(url.full = %RedactedUrl::from_url(&url), "resolved share url");

        Ok(url)
    }

    /// Ensures at least `REQUEST_INTERVAL` has passed since the previous request to Instagram,
    /// then records this one.
    ///
    /// The guard is intentionally held across the sleep: the mutex lock is the rate-limit queue.
    /// Concurrent callers block on `lock()` and take turns waiting out the remaining interval;
    /// dropping the guard before sleeping would release every caller at once into the same
    /// expired interval. Handler serialization makes the lock uncontended today, but this
    /// contract is what keeps a future concurrent caller from becoming a thundering herd.
    pub(super) async fn throttle(&self) {
        let mut last_request = self.last_request.lock().await;

        if let Some(last_request) = *last_request
            && let Some(wait) = REQUEST_INTERVAL.checked_sub(last_request.elapsed())
        {
            tokio::time::sleep(wait).await;
        }

        *last_request = Some(Instant::now());
    }
}
