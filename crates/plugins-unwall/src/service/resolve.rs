//! The cached submission path for an article.
//!
//! The database cache is consulted first, so every later post of an article is answered
//! without touching the API. A first submission is single-flight per URL — a burst of the same
//! article submits once — and attributed to the user who posted the link, whether or not the
//! submission succeeds.

use std::time::Duration;

use tracing::warn;
use url::Url;

use super::UnwallService;
use crate::error::Error;
use crate::model::{CachedUrl, NewFetch};
use zeta_plugin::Chain;

/// How long a submitted article stays in the in-flight cache, collapsing a burst of the same
/// article into a single submission.
pub(super) const INFLIGHT_TTL: Duration = Duration::from_mins(10);

impl UnwallService {
    /// Returns the cached reader link for `article`, submitting it otherwise.
    ///
    /// A first submission is single-flight per URL — a burst of the same article submits once
    /// — and attributed to the user who posted the link, whether or not the submission
    /// succeeds. The database cache is consulted before the submission, so every later post is
    /// answered without touching the API.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the cache could not be consulted or written, the errors
    /// of `UnwallClient::submit` otherwise, and [`Error::InvalidUrl`] if a submission reported
    /// as cached cannot be read back.
    pub async fn resolve(&self, article: &Url, fetch: NewFetch) -> Result<CachedUrl, Error> {
        let key = fetch.url.clone();

        if let Some(cached) = self.repo.cached_url(&key).await? {
            return Ok(cached);
        }

        let unwall_url = self.reader_url(article).ok_or(Error::InvalidUrl)?;

        self.inflight
            .get_or_refresh(key, || async {
                let submitted = self.client.submit(article).await;

                // The fetch was triggered either way; a failed attribution is logged, never
                // fatal to the submission.
                if let Err(error) = self.repo.record_fetch(&fetch).await {
                    warn!(
                        exception.message = %Chain(&error),
                        "could not record the unwall fetch"
                    );
                }

                submitted?;

                self.repo
                    .insert_url(&fetch.url, &fetch.host, unwall_url.as_str())
                    .await?;

                Ok(Some(
                    self.repo
                        .cached_url(&fetch.url)
                        .await?
                        .ok_or(Error::InvalidUrl)?,
                ))
            })
            .await
            .and_then(|cached| cached.ok_or(Error::InvalidUrl))
    }
}
