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

mod assertions;
#[cfg(feature = "context")]
mod context;
#[cfg(feature = "database")]
mod database;
mod fixtures;
mod http;
#[cfg(feature = "spans")]
mod spans;

pub use assertions::{assert_parses, assert_url_hosts_cover};
#[cfg(feature = "context")]
pub use context::{Recording, RecordingPlugin, mock_client, publish_recording, test_context};
#[cfg(feature = "database")]
pub use database::MIGRATOR;
pub use http::{assert_redactions, assert_timeout_redactions, refused_address};
#[cfg(feature = "http")]
pub use http::{not_found_server, refused_get, timeout_error, wiremock};
#[cfg(feature = "spans")]
pub use spans::{RecordedSpan, SpanRecorder};

/// Implementation details the exported test macros resolve through `$crate`.
///
/// Kept public so the macros work from any crate in the workspace; not part of the supported
/// API.
#[doc(hidden)]
pub mod __private {
    pub use serde_json;
}
