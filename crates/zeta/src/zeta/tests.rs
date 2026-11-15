//! Cross-cutting host lifecycle tests.
//!
//! Helper-specific tests live next to their helper: the signal tests in `signal` and the mirror
//! tests in `mirror`. The connection setup span is the host's telemetry boundary, so its
//! attribute test lives here rather than with the connection helpers.

use tracing::subscriber::with_default;
use tracing_subscriber::prelude::*;
use zeta_test_support::SpanRecorder;

use crate::config::IrcConfig;

use super::connection::connect_span;

#[test]
fn connect_span_records_server_and_network_attributes() {
    let recorder = SpanRecorder::default();
    let subscriber = tracing_subscriber::registry().with(recorder.clone());
    let config = IrcConfig {
        hostname: "irc.example.com".to_owned(),
        port: Some(6697),
        nickname: "zeta".to_owned(),
        ..IrcConfig::default()
    };

    with_default(subscriber, || drop(connect_span(&config)));

    let spans = recorder.spans();
    let span = spans
        .iter()
        .find(|span| span.name() == "irc.connect")
        .expect("the connection setup span should be recorded");

    assert_eq!(span.field("otel.kind"), Some("client"));
    assert_eq!(span.field("server.address"), Some("irc.example.com"));
    assert_eq!(span.field("server.port"), Some("6697"));
    assert_eq!(span.field("network.transport"), Some("tcp"));
    assert_eq!(span.field("network.protocol.name"), Some("irc"));
    assert_eq!(span.field("irc.nickname"), Some("zeta"));
}
