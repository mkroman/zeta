//! Tracing and telemetry configuration.
//!
//! [`TracingConfig`] gates the OpenTelemetry export and names the deployment environment that is
//! reported as `deployment.environment.name`.

use serde::Deserialize;

/// Tracing and logging configuration.
#[derive(Clone, Debug, Deserialize)]
pub struct TracingConfig {
    /// Enable tracing
    pub enabled: bool,
    /// The deployment environment reported as `deployment.environment.name`.
    ///
    /// Falls back to the `ZETA_ENVIRONMENT` environment variable when unset.
    #[serde(default)]
    pub environment: Option<String>,
}
