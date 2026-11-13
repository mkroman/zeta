//! The reply-task flow shared by the plugins that answer from a background task.
//!
//! A handler that fetches or resolves before it can reply must not block the message loop, but
//! spawning one task per event would let a burst of events fan out without bound. The plugins
//! therefore queue the work behind a semaphore, acquire the permit before the task exists, and
//! reply from inside the task through the client's sender.

use std::future::Future;
use std::sync::Arc;

use irc::client::{Client, Sender};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::Instrument;

/// Waits for one of `permits` before the reply task exists.
///
/// This is part of the [`spawn_reply_task!`](crate::spawn_reply_task) expansion; call the macro
/// instead of this function.
///
/// # Panics
///
/// Panics if `permits` was closed, which no plugin does.
#[doc(hidden)]
pub async fn acquire(permits: Arc<Semaphore>) -> OwnedSemaphorePermit {
    permits
        .acquire_owned()
        .await
        .expect("the reply task semaphore is never closed")
}

/// Spawns `work` as a reply task holding `permit`.
///
/// This is the body of [`spawn_reply_task!`](crate::spawn_reply_task); call the macro instead of
/// this function so the span gets its name.
///
/// The permit is moved into the task and released by its drop, on success, error and
/// cancellation alike. `work` receives the client's sender and the channel, and replies through
/// [`send_reply_via`](zeta_plugin::irc::send_reply_via), logging a failed send itself.
#[doc(hidden)]
pub fn spawn<F, Fut>(
    permit: OwnedSemaphorePermit,
    client: &Client,
    channel: &str,
    span: tracing::Span,
    work: F,
) where
    F: FnOnce(Sender, String) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    let sender = client.sender();
    let channel = channel.to_owned();

    tokio::spawn(
        async move {
            let _permit = permit;

            work(sender, channel).await;
        }
        .instrument(span),
    );
}

/// Spawns a task that replies to `channel`, waiting for one of `permits` first.
///
/// The span is named `$span` and carries the redacted URL: without it, the task's logs would
/// only ever reach stdout. It is a child of the caller's span, created while the caller is
/// current, so the work stays in the event's trace even though the handler returns (and its span
/// closes) first. The target stays under `zeta`: the span parents the HTTP client span, and
/// disabling it with a plugin log filter would leave that span as a trace root of its own.
///
/// `work` receives the client's sender and the channel, and must send its reply through
/// [`send_reply_via`](zeta_plugin::irc::send_reply_via), logging a failed send with `warn!`.
/// The returned future acquires the permit before the span is opened and the task is spawned, so
/// await it before the handler returns; the task itself keeps running in the background.
///
/// The span name is a literal because tracing metadata is static: a runtime `&str` cannot name
/// a span.
///
/// ```ignore
/// plugins_shared::spawn_reply_task!(
///     Arc::clone(&self.fetches),
///     client,
///     channel,
///     "titles.fetch",
///     &url,
///     move |sender, channel| async move {
///         let reply = render(&url).await;
///
///         if let Err(error) = send_reply_via(&sender, &channel, reply) {
///             warn!(exception.message = %error, "could not send the reply");
///         }
///     },
/// )
/// .await;
/// ```
#[macro_export]
macro_rules! spawn_reply_task {
    ($permits:expr, $client:expr, $channel:expr, $span:literal, $url:expr, $work:expr $(,)?) => {
        async move {
            let permit = $crate::task::acquire($permits).await;
            let span = $crate::task::__private::tracing::info_span!(
                target: "zeta.plugins",
                parent: $crate::task::__private::tracing::Span::current(),
                $span,
                url.full = %$crate::task::__private::RedactedUrlRef::from_url($url)
            );

            $crate::task::spawn(permit, $client, $channel, span, $work);
        }
    };
}

/// Items the [`spawn_reply_task!`](crate::spawn_reply_task) expansion needs, so invoking it does
/// not rely on the caller's dependencies.
#[doc(hidden)]
pub mod __private {
    pub use tracing;
    pub use zeta_url::RedactedUrlRef;
}
