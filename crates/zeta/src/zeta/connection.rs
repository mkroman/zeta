//! IRC connection setup and the incoming and outgoing paths.
//!
//! `connect_span` bounds the connection setup steps, `drive_outgoing` writes queued messages to
//! the socket independently of the dispatch loop, and `dispatch_messages` feeds incoming lines to
//! the plugin runtime until the stream ends.

use irc::client::prelude::{Capability, Client};
use tracing::{error, warn};

use crate::Error;
use crate::config::IrcConfig;
#[cfg(feature = "database")]
use std::sync::Arc;

use zeta_plugin_runtime::PluginRuntime;

use plugins::BundledPlugin;

/// Returns the span covering one IRC connection's setup.
///
/// The span is bounded to the setup steps — DNS resolution, the TCP/TLS connection, capability
/// negotiation and registration — and is not held for the connection's lifetime. The connection
/// itself is not a span: a process- or connection-lifetime span never closes, so the exporter
/// keeps buffering it. Every line and reply opens its own short span instead.
pub(super) fn connect_span(config: &IrcConfig) -> tracing::Span {
    tracing::info_span!(
        "irc.connect",
        otel.kind = "client",
        server.address = %config.hostname,
        server.port = u64::from(config.resolved_port()),
        network.transport = "tcp",
        network.protocol.name = "irc",
        irc.nickname = %config.nickname,
    )
}

/// Establishes the IRC connection: capability negotiation, registration, and the two side
/// paths (the outgoing sink and the stream).
///
/// The setup gets one bounded `irc.connect` span — the connection itself has no span; the
/// dispatcher and every reply open their own short spans, so nothing hangs from a never-closing
/// root.
///
/// # Errors
///
/// Returns an error when the client cannot be built, registration fails, or the stream cannot
/// be taken.
pub(super) async fn connect(
    irc: &IrcConfig,
) -> Result<
    (
        Client,
        irc::client::ClientStream,
        tokio::task::JoinHandle<()>,
    ),
    Error,
> {
    let mut client = Client::from_config(irc.into())
        .await
        .map_err(Error::IrcClient)?;

    // Request the IRCv3 capabilities the account subsystem runs on: message tags so servers
    // attach tags such as `msgid` to delivered messages, `account-tag` so logged-in senders
    // carry their services account on every message, `account-notify` so the bot learns when
    // users in its channels authenticate or log out (`ACCOUNT` notifications), and
    // `extended-join` so a join names the account joining. A server that supports none of
    // these answers with a `NAK`, which is harmless; registration continues, and users without
    // accounts then simply have no identity.
    client
        .send_cap_req(&[
            Capability::Custom("message-tags"),
            Capability::Custom("account-tag"),
            Capability::Custom("account-notify"),
            Capability::Custom("extended-join"),
        ])
        .map_err(Error::IrcRegistration)?;

    client.identify().map_err(Error::IrcRegistration)?;

    let outgoing = drive_outgoing(&mut client);

    let stream = client.stream()?;

    Ok::<_, Error>((client, stream, outgoing))
}

/// Drives the client's outgoing sink independently of the dispatch loop.
///
/// The sink is normally driven by the `ClientStream`'s polling, which would leave a queued
/// `QUIT` unflushed after the dispatch loop stops. Driving it separately writes sent messages
/// to the socket as they are queued.
///
/// Returns the task's handle so the caller can supervise the write path and wait for the final
/// flush after the last client handle is dropped.
///
/// # Panics
///
/// Panics if the outgoing future is unavailable — only possible if `stream()` had already been
/// called on the client.
pub(super) fn drive_outgoing(client: &mut Client) -> tokio::task::JoinHandle<()> {
    let outgoing = client
        .outgoing()
        .expect("the outgoing future is only available before the stream is taken");

    // The failure gets a short span of its own: a task-lifetime span would never be exported
    // until shutdown, and the OpenTelemetry layer needs a current span for the message.
    tokio::spawn(async move {
        if let Err(error) = outgoing.await {
            let span = tracing::error_span!("irc.outgoing.error");
            span.in_scope(
                || error!(exception.message = %error, "the IRC connection write path failed"),
            );
        }
    })
}

/// Dispatches incoming IRC messages as events to the plugins that registered interest, until
/// the stream ends.
///
/// # Errors
///
/// Returns any IRC protocol error that ends the stream, and [`Error::IrcDisconnected`] when the
/// server sends `ERROR` or closes the stream. A closed connection is reported as an error so the
/// process exits non-zero rather than looking like a successful run to its supervisor.
pub(super) async fn dispatch_messages(
    stream: &mut irc::client::ClientStream,
    runtime: &mut PluginRuntime<BundledPlugin>,
    #[cfg(feature = "database")] accounts: Option<&Arc<zeta_plugin::accounts::Accounts>>,
) -> Result<(), Error> {
    use futures::stream::StreamExt;

    while let Some(message) = stream.next().await.transpose()? {
        // `ERROR` carries the server's reason for closing the connection. It is not a `Raw`
        // command and is not routed, so the runtime never sees it; the host logs it in its own
        // `irc.error` span. The OpenTelemetry layer drops events that are not in the context of
        // a span, and the span is at info level, not debug, so an `info` filter cannot be what
        // keeps the reason out of the backend.
        //
        // Every line the dispatcher can deliver gets its `irc.dispatch` span — and the
        // `irc.raw` trace — from the dispatcher. PING, PONG, numerics and lines no subscription
        // can receive have no span.
        if let irc::proto::Command::ERROR(reason) = &message.command {
            let span = tracing::info_span!("irc.error", irc.command = "ERROR");
            span.in_scope(|| {
                warn!(
                    zeta.shutdown.reason = %reason,
                    "the IRC server closed the connection"
                );
            });

            return Err(Error::IrcDisconnected {
                reason: reason.clone(),
            });
        }

        // The account tracker sees every line before the plugins: it updates the in-memory
        // session state from the IRCv3 account notifications and tags, and touches no
        // database.
        #[cfg(feature = "database")]
        if let Some(accounts) = accounts {
            accounts.track(&message);
        }

        runtime.dispatch(message);
    }

    tracing::warn_span!("irc.stream.ended").in_scope(|| warn!("the IRC message stream ended"));

    Err(Error::IrcDisconnected {
        reason: "the server closed the connection".to_owned(),
    })
}
