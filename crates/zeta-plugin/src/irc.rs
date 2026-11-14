//! IRC reply helpers shared by bundled plugins.
//!
//! Plugin replies follow a common visual convention: a teal `>` marker (mIRC color 10), an
//! optional bold plugin name, and the message body with values emphasized by switching between
//! the reply color and the default color. The formatting helpers that produce those byte
//! sequences live in [`zeta_utils::irc`], next to the other IRC formatting helpers, and are
//! re-exported here so plugin code can keep importing them from this module. What remains here
//! are the helpers that need the plugin vocabulary: sending a finished reply without the copies
//! [`Client::send_privmsg`] makes, and replying with a failed argument parse's usage lines.

use std::future::Future;

use clap::Parser;
use irc::client::{Client, Sender};
use irc::proto::Command;

use crate::command::{ArgsError, ArgsMode};
use crate::event::CommandEvent;

pub use zeta_utils::irc::{
    BOLD, COLOR, PLAIN_REPLY_PREFIX, REPLY_MARKER, RESET, labeled_reply, labeled_reply_prefix,
    plain_reply, unstyle_text, usage_line, write_labeled_reply_prefix,
};

/// Sends each non-empty usage line as its own `PRIVMSG`.
///
/// Outlined from [`parse_or_usage`]: the loop is type-independent and only runs when the
/// argument parse fails, while the parse itself is monomorphised per argument type.
#[inline(never)]
fn reply_usage(
    client: &Client,
    channel: &str,
    error: &ArgsError,
    format: &dyn Fn(&str) -> String,
) -> Result<(), irc::error::Error> {
    for line in error.to_string().lines().filter(|line| !line.is_empty()) {
        send_reply(client, channel, format(line))?;
    }

    Ok(())
}

/// Parses the trailing arguments of `command` with the tokenization selected by `mode`,
/// replying with the usage lines to the command's channel when the parse fails.
///
/// Parse failures produce the command's usage or help output, which spans multiple lines; IRC
/// messages cannot contain line breaks, so each line is sent as its own `PRIVMSG`, formatted by
/// `format` (e.g. by wrapping it in [`labeled_reply`] with the plugin's label).
///
/// Returns `Ok(None)` when the usage was replied to and the command should be abandoned.
///
/// # Errors
///
/// Returns any error produced while sending the usage lines.
pub fn parse_or_usage<T: Parser + 'static>(
    client: &Client,
    command: &CommandEvent,
    mode: ArgsMode,
    format: impl Fn(&str) -> String,
) -> Result<Option<T>, irc::error::Error> {
    match command.spec.parse_with(command.args(), mode) {
        Ok(args) => Ok(Some(args)),
        Err(error) => {
            reply_usage(client, command.channel(), &error, &format)?;

            Ok(None)
        }
    }
}

/// Sends `message` to `channel` as a single `PRIVMSG` through the client's [`Sender`].
///
/// A thin wrapper over [`send_reply_via`] on [`Client::sender`], so both entry points share one
/// body and cannot drift apart. The enqueue is measured by an `irc.send` producer span, a child
/// of the current handler span when one is current.
///
/// # Errors
///
/// Returns any error produced while sending the message.
pub fn send_reply(
    client: &Client,
    channel: &str,
    message: String,
) -> Result<(), irc::error::Error> {
    send_reply_via(&client.sender(), channel, message)
}

/// Sends `message` to `channel` as a single `PRIVMSG` through `sender`, moving it into the
/// outgoing command.
///
/// A task that keeps a [`Sender`] alive after the handler returns can reply through this without
/// reaching for the [`Client`]. [`Client::send_privmsg`] formats its message into a `String`,
/// splits that and then copies every line again, so a reply built here would be copied twice
/// before it reaches the outgoing queue; sending the [`Command`] directly moves the reply
/// instead. A message that carries an IRC line break — which the reply builders pass through
/// verbatim, so only a hand-built message normally has one — still goes through
/// [`Sender::send_privmsg`], keeping its multi-line splitting unchanged.
///
/// `message` is an owned `String` rather than an `impl Into<String>` so this body is compiled
/// once instead of once per call-site argument type; callers that hold a `&str` allocate at the
/// call site with `to_owned`.
///
/// The enqueue is measured by an `irc.send` producer span, a child of the current handler span
/// when one is current.
///
/// # Errors
///
/// Returns any error produced while sending the message.
pub fn send_reply_via(
    sender: &Sender,
    channel: &str,
    message: String,
) -> Result<(), irc::error::Error> {
    let span = tracing::info_span!(
        target: "zeta.plugins",
        "irc.send",
        otel.kind = "producer",
        messaging.system.name = "irc",
        messaging.operation.name = "privmsg",
        messaging.operation.type = "send",
        messaging.destination.name = %channel,
        irc.channel = %channel,
    );
    let _guard = span.enter();

    if message.contains("\r\n") {
        return sender.send_privmsg(channel, message);
    }

    sender.send(Command::PRIVMSG(channel.to_owned(), message))
}

/// Awaits a lookup and replies with its outcome to `channel`.
///
/// `render` formats a successful value and `on_error` a failed lookup, so a handler that only
/// differs in how it presents the two does not repeat the `match` and [`send_reply`] scaffold.
///
/// # Errors
///
/// Returns any error produced while sending the reply.
pub async fn reply_lookup<T, E>(
    client: &Client,
    channel: &str,
    lookup: impl Future<Output = Result<T, E>>,
    render: impl FnOnce(T) -> String,
    on_error: impl FnOnce(E) -> String,
) -> Result<(), irc::error::Error> {
    let message = match lookup.await {
        Ok(value) => render(value),
        Err(error) => on_error(error),
    };

    send_reply(client, channel, message)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::Arc;
    use std::time::Duration;

    use clap::Parser;
    use irc::proto::Message;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::TcpListener;
    use tokio::time::timeout;

    use super::*;
    use crate::command::CommandSpec;

    /// How long the replies have to reach the test listener before the test gives up.
    const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

    /// Options whose required positional argument makes an empty invocation fail.
    #[derive(Debug, Parser)]
    /// Test options.
    struct Opts {
        /// the name to greet
        name: String,
    }

    const COMMAND: CommandSpec = CommandSpec::with_args::<Opts>(".greet", "Greet someone");

    /// Returns a client whose sends are accepted and discarded.
    async fn mock_client() -> Client {
        Client::from_config(irc::client::data::Config {
            nickname: Some("zeta-test".to_owned()),
            server: Some("mock".to_owned()),
            use_mock_connection: true,
            mock_initial_value: Some(String::new()),
            ..Default::default()
        })
        .await
        .expect("mock irc client")
    }

    /// Returns a command event invoking [`COMMAND`] with `args` in `#test`.
    fn command(args: &str) -> CommandEvent {
        let text = if args.is_empty() {
            COMMAND.trigger().to_owned()
        } else {
            format!("{} {args}", COMMAND.trigger())
        };
        let message = Message::new(Some("#test"), "PRIVMSG", vec!["#test", text.as_str()])
            .expect("message should parse");

        CommandEvent::new(Arc::new(message), COMMAND).expect("the message should match the command")
    }

    /// Returns the non-empty lines of the usage output for `args`.
    fn usage_lines(args: &str) -> Vec<String> {
        COMMAND
            .parse_with::<Opts>(args, ArgsMode::Shell)
            .expect_err("the required argument is missing")
            .to_string()
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn send_reply_and_send_reply_via_deliver_replies() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the test listener");
        let address = listener.local_addr().expect("the listener has an address");

        // The mock connection's sent log is `#[cfg(test)]` inside the irc crate, so the replies
        // are asserted on a real socket instead: a plain reply, a reply carrying an IRC line
        // break, and a direct `Sender` reply.
        let mut client = Client::from_config(irc::client::data::Config {
            nickname: Some("zeta-test".to_owned()),
            server: Some(address.ip().to_string()),
            port: Some(address.port()),
            // `use_tls` defaults to true when a TLS feature is enabled, which would hang the
            // handshake against this plain listener.
            use_tls: Some(false),
            ..Default::default()
        })
        .await
        .expect("the client connects to the test listener");

        // Without the outgoing future being driven, the enqueued commands never reach the socket.
        let outgoing = client.outgoing().expect("the outgoing future is available");
        tokio::spawn(async move {
            let _ = outgoing.await;
        });

        send_reply(&client, "#test", "plain".to_owned()).expect("the plain reply sends");
        send_reply(&client, "#test", "a\r\nb".to_owned()).expect("the multi-line reply sends");
        send_reply_via(&client.sender(), "#test", "via sender".to_owned())
            .expect("the direct-sender reply sends");

        let (socket, _) = listener.accept().await.expect("the client connects");
        let mut reader = BufReader::new(socket);
        let mut delivered = Vec::new();
        for _ in 0..4 {
            let mut line = String::new();
            timeout(DELIVERY_TIMEOUT, reader.read_line(&mut line))
                .await
                .expect("every reply reaches the socket in time")
                .expect("every reply reads as a line");
            delivered.push(line);
        }

        assert_eq!(
            delivered,
            [
                "PRIVMSG #test plain\r\n",
                "PRIVMSG #test a\r\n",
                "PRIVMSG #test b\r\n",
                "PRIVMSG #test :via sender\r\n",
            ],
            "the line-break reply splits into two PRIVMSGs, all in send order"
        );
    }

    #[tokio::test]
    async fn replies_with_each_non_empty_usage_line_in_order() {
        let client = mock_client().await;
        let command = command("");
        let sent = RefCell::new(Vec::new());

        let result = parse_or_usage::<Opts>(&client, &command, ArgsMode::Shell, |line| {
            sent.borrow_mut().push(line.to_owned());

            format!("> {line}")
        })
        .expect("the mock client accepts the messages");

        assert!(result.is_none(), "a failed parse abandons the command");
        assert_eq!(
            *sent.borrow(),
            usage_lines(""),
            "one message per non-empty line"
        );
    }
}
