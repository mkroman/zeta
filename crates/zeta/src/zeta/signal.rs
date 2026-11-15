//! Signal-driven graceful shutdown.
//!
//! `shutdown_signal` resolves on the first shutdown signal, `note_signal` logs it and arms the
//! forced exit, and `exit_on_second_signal` ends the process immediately when another signal
//! arrives during the shutdown grace.

use tracing::{info, warn};

/// The signal that triggered a graceful shutdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ShutdownSignal {
    /// `SIGINT`, sent by `Ctrl-C`.
    Interrupt,
    /// `SIGTERM`, sent by a service manager or container orchestrator.
    Terminate,
}

impl ShutdownSignal {
    /// Returns the signal's conventional name, for logs.
    const fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        }
    }

    /// Returns the process exit code conventionally associated with the signal.
    const fn exit_code(self) -> i32 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
        }
    }
}

/// Returns the first shutdown signal received by the process.
///
/// `SIGINT` (`Ctrl-C`) and, on unix, `SIGTERM` — e.g. sent by a container orchestrator deleting
/// the pod — both trigger shutdown. The `SIGTERM` listener is best-effort: when it cannot be
/// installed, only `SIGINT` triggers shutdown.
pub(super) async fn shutdown_signal() -> ShutdownSignal {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        // Installation failure is ignored; shutdown still works through `SIGINT`.
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let sigterm = async move { sigterm.recv().await };
            tokio::pin!(sigterm);

            tokio::select! {
                _ = tokio::signal::ctrl_c() => return ShutdownSignal::Interrupt,
                _ = &mut sigterm => return ShutdownSignal::Terminate,
            }
        }
    }

    if tokio::signal::ctrl_c().await.is_err() {
        tracing::warn_span!("zeta.signal.install")
            .in_scope(|| warn!("failed to install the SIGINT listener; shutting down anyway"));
    }

    ShutdownSignal::Interrupt
}

/// Exits the process immediately when a second shutdown signal arrives.
///
/// The signal handlers stay installed after the first signal is received, so a second one
/// resolves here instead of terminating the process with the default handler.
pub(super) fn exit_on_second_signal() {
    tokio::spawn(async move {
        let signal = shutdown_signal().await;

        // The span closes as soon as the message is logged; the task then exits the process.
        let span = tracing::warn_span!("zeta.signal.second", signal = signal.name());
        span.in_scope(|| warn!("received a second shutdown signal; exiting now"));

        std::process::exit(signal.exit_code());
    });
}

/// Logs the signal that triggered the shutdown and arms the forced exit for a second one.
///
/// `run` has no process-lifetime span of its own, and the OpenTelemetry layer drops events that
/// are not in a span, so the one-off diagnostic gets a short span here.
pub(super) fn note_signal(signal: ShutdownSignal) {
    tracing::info_span!("zeta.signal", signal = signal.name()).in_scope(|| info!("shutting down"));

    // Another signal during the grace period forces an immediate exit.
    exit_on_second_signal();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_signal_responds_to_sigterm() {
        use futures::task::noop_waker;
        use std::task::Poll;

        let mut signal = Box::pin(shutdown_signal());

        // Poll once with a no-op waker: the first poll installs the `SIGINT` and `SIGTERM`
        // listeners, so the signal below can never arrive while the default disposition is
        // still active — which would terminate the whole test process.
        let waker = noop_waker();
        let mut context = std::task::Context::from_waker(&waker);

        assert_eq!(signal.as_mut().poll(&mut context), Poll::Pending);

        std::process::Command::new("kill")
            .args(["-s", "TERM", &std::process::id().to_string()])
            .status()
            .expect("could not send SIGTERM");

        let received = signal.await;

        assert_eq!(received, ShutdownSignal::Terminate);
    }
}
