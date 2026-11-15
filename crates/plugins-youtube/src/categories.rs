//! The background refresh of the cached YouTube video categories.
//!
//! The task started on load ticks once per [`CATEGORIES_CHECK_INTERVAL`] and only fetches the
//! categories when the cache is missing or past its [`CATEGORIES_TTL`], so the API is called at
//! most once per TTL and a failed fetch is retried on the next tick.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::MissedTickBehavior;
use tracing::{Instrument, debug, warn};

use zeta_plugin::Chain;

use super::YouTube;
use super::api::fetch_video_categories;

/// The time-to-live of the cached video categories.
pub const CATEGORIES_TTL: Duration = Duration::from_hours(24);

/// How often the categories task rechecks the cache for staleness.
///
/// A tick only fetches when the cached entry is missing or past its TTL, so the actual API
/// calls happen at most once per [`CATEGORIES_TTL`]; the shorter check interval makes a failed
/// fetch retry within an hour instead of after a full TTL.
const CATEGORIES_CHECK_INTERVAL: Duration = Duration::from_hours(1);

impl YouTube {
    /// Spawns the task that periodically refreshes the cached video categories.
    ///
    /// The task ticks once per [`CATEGORIES_CHECK_INTERVAL`], skipping missed ticks: a tick is
    /// only an actual API call when the cached entry is missing or past its TTL, so the
    /// categories are fetched at most once per [`CATEGORIES_TTL`] and a failed fetch is retried
    /// on the next tick. Refresh failures are logged and leave the stale map in place.
    pub(super) fn start_categories_refresh(&self) {
        let (client, api_key, region_code, cache) = (
            self.client.clone(),
            self.api_key.clone(),
            self.region_code.clone(),
            Arc::clone(&self.video_categories),
        );

        tokio::spawn(async move {
            tracing::info_span!("video_category.refresh.start").in_scope(|| {
                debug!("starting video category refresh task");
            });

            // The first tick completes immediately, populating the cache at startup.
            let mut interval = tokio::time::interval(CATEGORIES_CHECK_INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                async {
                    let refreshed = cache
                        .refresh(|| async {
                            fetch_video_categories(&client, &api_key, &region_code)
                                .await
                                .map(Arc::new)
                        })
                        .await;

                    if let Err(error) = refreshed {
                        warn!(
                            exception.message = %Chain(&error),
                            "could not refresh video categories"
                        );
                    }
                }
                // The target stays under `zeta`: the tick parents the HTTP calls, and a plugin
                // log filter must not leave them as trace roots of their own.
                .instrument(tracing::info_span!(
                    target: "zeta.plugins",
                    "video_category.refresh"
                ))
                .await;
            }
        });
    }
}
