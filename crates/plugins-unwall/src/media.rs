//! Article submission to unwall.app and the reply for a failed resolution.

use std::sync::Arc;

use tracing::warn;

use zeta_plugin::database::Attribution;
use zeta_plugin::irc::{plain_reply, send_reply_via};
use zeta_plugin::{Chain, Client, UrlEvent};

use super::model::NewFetch;
use super::{Error, Unwall};

/// The identifier of the network recorded with every fetch.
// TODO: support multiple networks dynamically (as `.ofn` does).
const NETWORK_ID: &str = "irc.rwx.im:6697";

/// The maximum number of article submissions that run at once.
///
/// A first mirror runs server-side for up to `submit_timeout`, so one message with many covered
/// links must not fan out into an unbounded number of requests against the rate-limited API.
pub(crate) const SUBMIT_CONCURRENCY: usize = 4;

impl Unwall {
    /// Submits a covered article to unwall.app in a task of its own, replying with the reader
    /// link once the mirror is warm — or from the cache, when the article is known.
    ///
    /// Waits for one of [`SUBMIT_CONCURRENCY`] permits before the task is spawned, so a message
    /// with many covered links queues in the handler instead of spawning an unbounded number of
    /// tasks.
    pub(super) async fn submit(&self, client: &Client, event: &UrlEvent) {
        let url = event.url();

        if !matches!(url.scheme(), "http" | "https") {
            return;
        }

        let Some(host) = url.host_str() else {
            return;
        };

        if !self.service.covers(host) {
            return;
        }

        let Some(sender) = event.sender() else {
            return;
        };

        // The fragment is not part of what unwall mirrors, so it is not part of the cache key
        // either.
        let mut article = url.clone();
        article.set_fragment(None);

        let fetch = NewFetch {
            url: article.as_str().to_owned(),
            host: host.to_owned(),
            attribution: Attribution::from_sender(sender, event.channel()),
            network_id: NETWORK_ID.to_owned(),
        };

        let service = Arc::clone(&self.service);

        plugins_shared::spawn_reply_task!(
            Arc::clone(&self.submits),
            client,
            event.channel(),
            "unwall.resolve",
            &article,
            move |irc, channel| async move {
                match service.resolve(&article, fetch).await {
                    Ok(cached) => {
                        // The link rides on the plain reply prefix, which colors the
                        // whole line cyan.
                        if let Err(error) =
                            send_reply_via(&irc, &channel, plain_reply(&cached.unwall_url))
                        {
                            warn!(exception.message = %error, "could not send the unwall link");
                        }
                    }
                    Err(error) => {
                        warn!(
                            exception.message = %Chain(&error),
                            "could not unwall the article"
                        );

                        let reply = resolve_reply(&error);

                        if let Err(error) = send_reply_via(&irc, &channel, plain_reply(reply)) {
                            warn!(exception.message = %error, "could not send the unwall failure");
                        }
                    }
                }
            },
        )
        .await;
    }
}

/// Formats a failed article resolution.
///
/// A rejected article, an unusable URL, and a reply-grade transport failure reuse the error's
/// own user-facing text; every other failure is reduced to the context, because the caller logs
/// the error.
pub(crate) fn resolve_reply(error: &Error) -> String {
    match error {
        Error::Status(_) | Error::InvalidUrl | Error::Request(_) => error.to_string(),
        Error::Database(_) | Error::BaseUrl(_) => "could not unwall the article".to_owned(),
    }
}
