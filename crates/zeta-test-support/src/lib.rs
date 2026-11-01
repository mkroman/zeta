//! Test utilities shared by the zeta workspace crates.
//!
//! A leaf crate with purpose: the standalone client crates cannot depend on `zeta` — the
//! dependency direction runs the other way — so the helpers here stick to the workspace's
//! external dependencies and to `zeta-plugin` (never the host crate itself). Everything that
//! constructs a `zeta` host type stays in the consuming crate's own test module instead.
//!
//! The connection-refused helpers use a reserved local address rather than wiremock on purpose:
//! a mock server cannot refuse a connection while it is running, and a listener dropped before
//! connecting is racy — the kernel can hand the port to another test in between. TCP port 0 is
//! reserved and can never have a listener (`bind` treats a zero port as "pick an ephemeral
//! port"), so connecting to it always fails in the connect phase: Linux refuses it, while other
//! kernels reject the address. Wiremock is re-exported for everything else.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
#[cfg(feature = "http")]
use std::time::Duration;

use url::Url;

#[cfg(feature = "http")]
pub use wiremock;

/// Constructs a plugin [`Context`](zeta_plugin::Context) for tests: a default configuration
/// and an unreachable (lazily connected) database pool.
///
/// # Panics
///
/// Panics if the lazy database pool cannot be constructed, which only happens with a
/// misconfigured sqlx build.
#[cfg(feature = "context")]
#[must_use]
pub fn test_context() -> zeta_plugin::Context {
    zeta_plugin::Context::new(
        {
            use sqlx::postgres::PgPoolOptions;

            PgPoolOptions::new()
                .connect_lazy("postgresql://invalid/zeta_test")
                .expect("lazy database pool")
        },
        zeta_plugin::config::HostConfig::default(),
    )
}

/// The lifecycle log shared between a test and its [`RecordingPlugin`].
#[cfg(feature = "context")]
pub type Recording = std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>;

/// Publishes a fresh recording log through the context's shared state.
///
/// The returned handle observes the lifecycle events [`RecordingPlugin`] appends.
///
/// # Panics
///
/// Panics if the context already holds a recording log.
#[cfg(feature = "context")]
pub fn publish_recording(ctx: &zeta_plugin::Context) -> Recording {
    let recording: Recording = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

    assert!(
        ctx.shared
            .publish(std::sync::Arc::clone(&recording))
            .is_none()
    );

    recording
}

/// A plugin recording the lifecycle events it observes in a shared log.
///
/// It registers one command (`.recording`) so registries can exercise command publication, and
/// appends `"message"` and `"shutdown"` to the log from the matching handlers.
#[cfg(feature = "context")]
pub struct RecordingPlugin {
    /// The log the plugin appends to, shared with the test.
    pub recording: Recording,
}

#[cfg(feature = "context")]
#[zeta_plugin::async_trait::async_trait]
impl zeta_plugin::Plugin for RecordingPlugin {
    async fn handle_message(
        &self,
        _: &zeta_plugin::Context,
        _: &zeta_plugin::Client,
        _: &zeta_plugin::MessageEvent,
    ) -> Result<(), zeta_plugin::Error> {
        self.recording
            .lock()
            .expect("recording log lock")
            .push("message");

        Ok(())
    }

    async fn shutdown(
        &mut self,
        _: &zeta_plugin::Context,
        _: &zeta_plugin::Client,
    ) -> Result<(), zeta_plugin::Error> {
        self.recording
            .lock()
            .expect("recording log lock")
            .push("shutdown");

        Ok(())
    }
}

#[cfg(feature = "context")]
impl zeta_plugin::Registered for RecordingPlugin {
    const NAME: &'static str = "recording";

    type Settings = zeta_plugin::NoSettings;

    fn new(
        ctx: &zeta_plugin::Context,
        _: &zeta_plugin::NoSettings,
        subscriptions: &mut zeta_plugin::Subscriptions,
    ) -> Result<Self, zeta_plugin::Error> {
        subscriptions.command(zeta_plugin::CommandSpec::new(
            ".recording",
            "record a message",
        ));

        let recording = ctx
            .shared
            .get::<std::sync::Mutex<Vec<&'static str>>>()
            .expect("recording log should be published through `publish_recording`");

        Ok(Self { recording })
    }
}

/// How long the wiremock server behind [`timeout_error`] waits before answering: long enough
/// that a client with [`TIMEOUT_CLIENT`] cannot possibly get a response.
#[cfg(feature = "http")]
const LATE_RESPONSE: Duration = Duration::from_secs(5);

/// The client timeout [`timeout_error`] requests with: the response cannot arrive within it.
#[cfg(feature = "http")]
const TIMEOUT_CLIENT: Duration = Duration::from_millis(50);

/// Returns the local address whose connections are refused.
///
/// The address names TCP port 0: IANA reserves it ([RFC 6335]), and a zero port in `bind` means
/// "pick an ephemeral port", so no listener can ever occupy it. A connection to it therefore
/// always fails in the connect phase, without this helper holding a socket — while URLs built
/// against the address can still carry credential-looking queries. A fixed nonzero port would
/// only be free by convention and could belong to another test.
///
/// [RFC 6335]: https://www.rfc-editor.org/rfc/rfc6335#section-6
#[must_use]
pub const fn refused_address() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

/// Sends a GET with the given query string to [`refused_address`].
///
/// Returns the transport error and the address: a real connection failure whose URL still
/// carries a credential-looking query, for testing the wrappers' redaction guarantees.
///
/// # Panics
///
/// Panics if the request does not fail, or if reqwest does not attach the request URL to the
/// error.
#[cfg(feature = "http")]
pub async fn refused_get(query: &str) -> (reqwest::Error, SocketAddr) {
    let address = refused_address();

    let error = reqwest::Client::new()
        .get(format!("http://{address}/?{query}"))
        .send()
        .await
        .expect_err("port zero has no listener");

    // Sanity: reqwest attaches the request URL to the error — redacting it is on us.
    assert!(error.url().is_some(), "reqwest attaches the request url");

    (error, address)
}

/// Sends a GET with a credential-looking query against a wiremock server that answers only
/// after the request's client timeout has expired, returning the timeout error and the address.
///
/// # Panics
///
/// Panics if the request fails with anything but a timeout.
#[cfg(feature = "http")]
pub async fn timeout_error() -> (reqwest::Error, SocketAddr) {
    // The server is exclusive: the delayed response keeps the request in flight after the
    // client times out and the test returns. A pooled server would be recycled with that late
    // request still on the wire, and the next test to borrow it would record it as its own.
    let server = wiremock::MockServer::builder().start().await;

    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(200).set_delay(LATE_RESPONSE))
        .mount(&server)
        .await;

    let client = reqwest::Client::builder()
        .timeout(TIMEOUT_CLIENT)
        .build()
        .expect("build the http client");

    let error = client
        .get(format!("http://{}?key=secret", server.address()))
        .send()
        .await
        .expect_err("the request should hit the client timeout");

    assert!(error.is_timeout(), "expected a timeout, got: {error}");

    (error, *server.address())
}

/// Asserts the redaction guarantees a connection-refused wrapper error makes.
///
/// No rendering leaks `query`; the short form (what a channel reply gets) is classified,
/// host-only and bounded; the full rendering (what a log line gets) carries the redacted URL.
///
/// # Panics
///
/// Panics on the first broken guarantee.
pub fn assert_redactions(
    message: &str,
    full: &str,
    debug: &str,
    address: &SocketAddr,
    query: &str,
) {
    assert_common(message, full, debug, address, query, "could not connect");

    // The source chain carries hyper's `client error (Connect)` link on every platform. The
    // OS wording below it differs (Linux says "Connection refused", macOS says "Address not
    // available" for the reserved port), so only Linux can assert on it.
    assert!(full.contains("client error (Connect)"), "{full}");

    #[cfg(target_os = "linux")]
    assert!(full.contains("refused"), "{full}");
}

/// The timeout counterpart of [`assert_redactions`].
///
/// The timeout short form instead of the connection one, and no cause-chain word to lean on.
///
/// # Panics
///
/// Panics on the first broken guarantee.
pub fn assert_timeout_redactions(
    message: &str,
    full: &str,
    debug: &str,
    address: &SocketAddr,
    query: &str,
) {
    assert_common(message, full, debug, address, query, "request timed out");
}

/// Asserts the guarantees shared by [`assert_redactions`] and [`assert_timeout_redactions`].
fn assert_common(
    message: &str,
    full: &str,
    debug: &str,
    address: &SocketAddr,
    query: &str,
    short_prefix: &str,
) {
    assert!(!message.contains(query), "{message}");
    assert!(message.starts_with(short_prefix), "{message}");
    assert!(message.contains(&address.ip().to_string()), "{message}");
    assert!(message.len() < 64, "{message}");

    assert!(!full.contains(query), "{full}");
    assert!(full.contains(&format!("http://{address}/")), "{full}");

    assert!(!debug.contains(query), "{debug}");
}

/// Starts a wiremock server answering `404 Not Found` to every request.
///
/// An S3-compatible endpoint where the object check reports the object as absent and the upload
/// then fails with the non-retryable status.
#[cfg(feature = "http")]
pub async fn not_found_server() -> wiremock::MockServer {
    let server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(404))
        .mount(&server)
        .await;

    server
}

/// Reads a fixture file from the calling crate's `tests/fixtures` directory.
///
/// The path is resolved against the crate that invokes the macro, so plugins can keep their
/// fixtures next to their tests without repeating the manifest-directory dance.
///
/// # Panics
///
/// Panics if the fixture cannot be read, naming the resolved path and the cause.
#[macro_export]
macro_rules! fixture {
    ($path:expr) => {{
        let path = ::std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join($path);

        ::std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("could not read fixture {}: {error}", path.display()))
    }};
}

/// Asserts that `parse` classifies every `input` of `cases` as its expected result.
///
/// Names the input when an assertion fails.
///
/// # Panics
///
/// Panics if a case's URL cannot be parsed or an expectation fails.
pub fn assert_parses<T: std::fmt::Debug + PartialEq>(
    parse: impl Fn(&Url) -> T,
    cases: &[(&str, T)],
) {
    for (input, expected) in cases {
        let url = Url::parse(input).expect("test URL parses");

        assert_eq!(&parse(&url), expected, "for {input}");
    }
}

/// Generates the three settings tests shared by every plugin with a configuration section.
///
/// Expands to a `default_settings` test asserting the values produced by [`Default`], a
/// `settings_deserialize` test deserializing the given JSON object before asserting on it, and
/// a `settings_ignore_unknown_keys` test pinning that unknown keys are ignored instead of
/// rejected — removing a setting must never crash deployments that still name it.
#[macro_export]
macro_rules! settings_tests {
    (
        $ty:ty, $settings:ident,
        default: { $($default:tt)* }
        deserialize: { $($json:tt)* } assert: { $($assert:tt)* }
    ) => {
        #[test]
        fn default_settings() {
            let $settings = <$ty>::default();

            $($default)*
        }

        #[test]
        fn settings_deserialize() {
            let $settings: $ty = serde_json::from_value(serde_json::json!({ $($json)* }))
                .expect("could not deserialize settings");

            $($assert)*
        }

        #[test]
        fn settings_ignore_unknown_keys() {
            let mut value = serde_json::json!({ $($json)* });
            value
                .as_object_mut()
                .expect("the settings fixture should be an object")
                .insert("setting_removed_in_this_version".to_owned(), serde_json::Value::Bool(true));

            serde_json::from_value::<$ty>(value)
                .expect("unknown settings keys must be ignored, never rejected");
        }
    };
}

/// Asserts that the expression is empty, naming the value in the panic.
///
/// A plain `assert!(value.is_empty())` only prints the condition on failure — useless for
/// finding out what the stray entries were, which is the reasoning behind clippy's
/// `assert_is_empty` lint — while the `assert_eq!` alternative needs brittle explicit types
/// (`Vec::<String>::new()`, because `[]` does not typecheck for a `Vec<String>`). This macro
/// prints the value without either problem.
///
/// An optional message follows the value, formatted like any `format!` invocation.
///
/// # Panics
///
/// Panics if the expression is not empty, printing its `Debug` rendering and, when given,
/// the custom message.
#[macro_export]
macro_rules! assert_empty {
    ($value:expr $(,)?) => {
        match &$value {
            value if value.is_empty() => {}
            value => panic!("assertion failed: value is empty\n  value: {value:?}"),
        }
    };
    ($value:expr, $($message:tt)+) => {
        match &$value {
            value if value.is_empty() => {}
            value => panic!(
                "assertion failed: value is empty\n  value: {value:?}\n{}",
                format_args!($($message)+)
            ),
        }
    };
}

#[cfg(test)]
mod tests {
    // The gate is load-bearing: with `http` off, no test below uses anything from `super`, and
    // a fully-unused glob import warns.
    #[cfg(feature = "http")]
    use super::*;

    #[test]
    fn assert_empty_passes_on_empty_and_names_the_value_on_failure() {
        assert_empty!(Vec::<String>::new());
        let empty: [u8; 0] = [];
        assert_empty!(empty);

        let panic = std::panic::catch_unwind(|| assert_empty!(vec!["stray entry"]))
            .expect_err("non-empty values must panic");
        let message = panic
            .downcast_ref::<String>()
            .expect("the panic payload is a string");

        assert!(message.contains("value is empty"), "{message}");
        assert!(message.contains(r#""stray entry""#), "{message}");
    }

    #[test]
    fn assert_empty_appends_the_custom_message_to_the_panic() {
        let panic = std::panic::catch_unwind(|| {
            let channel = "#zeta";

            assert_empty!(vec!["stray"], "{channel} must have no strays");
        })
        .expect_err("non-empty values must panic");
        let message = panic
            .downcast_ref::<String>()
            .expect("the panic payload is a string");

        assert!(message.contains("#zeta must have no strays"), "{message}");
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn refused_get_fails_to_connect_with_the_query_attached() {
        let (error, address) = refused_get("login_token=secret").await;

        assert!(error.is_connect(), "{error}");
        assert!(
            error
                .url()
                .unwrap()
                .as_str()
                .ends_with("?login_token=secret"),
            "{}",
            error.url().unwrap()
        );
        // The URL keeps the reserved port instead of normalizing it away: that is the whole
        // refusal strategy.
        assert!(
            error
                .url()
                .unwrap()
                .as_str()
                .starts_with("http://127.0.0.1:0/"),
            "{}",
            error.url().unwrap()
        );
        assert_eq!(address.ip().to_string(), "127.0.0.1");
        assert_eq!(address.port(), 0);
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn timeout_error_times_out_with_the_query_attached() {
        let (error, address) = timeout_error().await;

        assert!(error.is_timeout(), "{error}");
        assert!(error.url().unwrap().as_str().ends_with("?key=secret"));
        assert_eq!(address.ip().to_string(), "127.0.0.1");
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn not_found_server_answers_every_request_with_a_404() {
        let server = not_found_server().await;

        for method in [
            reqwest::Method::GET,
            reqwest::Method::HEAD,
            reqwest::Method::PUT,
        ] {
            let response = reqwest::Client::new()
                .request(
                    method.clone(),
                    format!("http://{}/bucket/key", server.address()),
                )
                .send()
                .await
                .expect("the server responds");

            assert_eq!(
                response.status(),
                reqwest::StatusCode::NOT_FOUND,
                "{method}"
            );
        }

        let requests = server
            .received_requests()
            .await
            .expect("requests are tracked");

        assert_eq!(requests.len(), 3);
    }
}
