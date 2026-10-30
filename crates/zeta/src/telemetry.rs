//! Tracing and OpenTelemetry initialization.

use std::env;

use miette::{IntoDiagnostic, WrapErr};
use opentelemetry::InstrumentationScope;
use opentelemetry::trace::TracerProvider;
use opentelemetry_resource_detectors::{
    HostResourceDetector, K8sResourceDetector, OsResourceDetector,
};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::resource::{EnvResourceDetector, ResourceDetector};
use opentelemetry_sdk::runtime::Tokio;
use opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor;
use opentelemetry_semantic_conventions::SCHEMA_URL;
use opentelemetry_semantic_conventions::resource::{DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_VERSION};
use tracing::info;

use zeta::config;

/// Returns a list of resource detectors to use to enrich OpenTelemetry attributes.
fn otel_resource_detectors() -> Vec<Box<dyn ResourceDetector>> {
    vec![
        Box::new(EnvResourceDetector::default()),
        Box::new(OsResourceDetector),
        Box::new(HostResourceDetector::default()),
        Box::new(K8sResourceDetector),
    ]
}

/// Flushes and shuts down the OpenTelemetry tracer provider when dropped.
///
/// The provider's batch span processor holds spans in memory until it is shut down, so without
/// this guard the spans emitted while the bot shuts down — exactly the ones worth keeping — are
/// lost when the process exits. Hold the guard for the lifetime of the process.
///
/// The SDK shuts a provider down when its last reference drops, but that never happens here:
/// every `SdkTracer` holds a provider clone, and the tracer lives in the global subscriber,
/// which is a static. The explicit shutdown is therefore required.
///
/// `shutdown` blocks while the batch processor flushes. The multi-thread runtime this binary
/// uses runs the exporter on another worker meanwhile; on a current-thread runtime, shut down
/// from `tokio::task::spawn_blocking` instead to avoid deadlocking with the exporter task.
#[must_use = "dropping the guard immediately shuts the tracer provider down"]
pub struct TelemetryGuard {
    /// The provider to shut down, or `None` when OpenTelemetry is disabled.
    provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        let Some(provider) = self.provider.take() else {
            return;
        };

        if let Err(error) = provider.shutdown() {
            eprintln!("could not shut down the OpenTelemetry tracer provider: {error}");
        }
    }
}

pub fn try_init(tracing: &config::TracingConfig) -> miette::Result<TelemetryGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    // The deployment environment is a resource attribute, not a per-span field. The configured
    // value wins over `ZETA_ENVIRONMENT`; an empty value is treated as unset.
    let environment = tracing
        .environment
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            env::var("ZETA_ENVIRONMENT")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        });

    // Create a tracing layer with the configured tracer, keeping the provider so the guard can
    // flush the batches on exit.
    let (telemetry_layer, provider) = if tracing.enabled {
        // Set up the OTLP exporter
        let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .build()
            .into_diagnostic()
            .wrap_err("building otlp http exporter failed")?;
        // Set up resource detectors to enrich otel attributes
        let res_detectors = otel_resource_detectors();

        let mut resource = Resource::builder_empty()
            // `service.name` and `service.version` are the resource attributes every consumer
            // groups by: https://opentelemetry.io/docs/specs/semconv/registry/attributes/service/
            .with_service_name(env!("CARGO_PKG_NAME"))
            .with_attribute(opentelemetry::KeyValue::new(
                SERVICE_VERSION,
                env!("CARGO_PKG_VERSION"),
            ))
            .with_detectors(&res_detectors);

        if let Some(environment) = environment {
            resource = resource.with_attribute(opentelemetry::KeyValue::new(
                DEPLOYMENT_ENVIRONMENT_NAME,
                environment,
            ));
        }

        // The schema URL identifies the semantic-convention version the resource follows, and
        // must be a retrievable schema file:
        // https://opentelemetry.io/docs/specs/otel/schemas/#schema-url
        let resource = resource
            .with_schema_url(None::<opentelemetry::KeyValue>, SCHEMA_URL)
            .build();

        // Resource detectors for tracing context. The processor spawns its batching task on the
        // tokio runtime, so the async exporter can be driven from it.
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_span_processor(BatchSpanProcessor::builder(otlp_exporter, Tokio).build())
            .with_resource(resource)
            .build();
        let scope = InstrumentationScope::builder(env!("CARGO_PKG_NAME"))
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_schema_url(SCHEMA_URL)
            .build();
        let tracer = provider.tracer_with_scope(scope);
        let layer = tracing_opentelemetry::layer().with_tracer(tracer);

        (Some(layer), Some(provider))
    } else {
        (None, None)
    };

    let stdout_layer = tracing_subscriber::fmt::layer().json();

    // initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| DEFAULT_FILTER.into()),
        )
        .with(telemetry_layer)
        .with(stdout_layer)
        .try_init()
        .into_diagnostic()
        .wrap_err("could not init registry")?;

    info!("tracing initialized");

    Ok(TelemetryGuard { provider })
}

/// The filter used when `RUST_LOG` is not set.
///
/// `zeta=debug` would prefix-match every `zeta_*` crate and enable the per-message and
/// per-progress debug lines; production starts at info and the debug detail is opt-in with
/// `RUST_LOG`. `plugins=info` keeps the plugin spans (and the HTTP, DNS and S3 children under
/// them) in the export: without them the client spans would become roots of their own traces.
const DEFAULT_FILTER: &str =
    "zeta=info,plugins=info,reddit=debug,dendanskeordbog=debug,kagi=debug,coinmarketcap=debug";

#[cfg(test)]
mod tests {
    use tracing::Level;
    use tracing_subscriber::layer::SubscriberExt;

    use super::DEFAULT_FILTER;

    /// The filter that keeps one message one trace must keep the plugin spans that parent the
    /// HTTP, DNS and S3 client spans, without enabling their per-operation debug detail.
    #[test]
    fn the_default_filter_exports_plugin_spans_but_not_their_detail() {
        let subscriber =
            tracing_subscriber::registry().with(tracing_subscriber::EnvFilter::new(DEFAULT_FILTER));

        tracing::subscriber::with_default(subscriber, || {
            assert!(
                !tracing::span!(target: "plugins_titles::lib", Level::INFO, "titles.fetch")
                    .is_disabled()
            );
            assert!(
                !tracing::span!(target: "zeta_http::client", Level::INFO, "http.client")
                    .is_disabled()
            );
            assert!(
                !tracing::span!(
                    target: "zeta_plugin_runtime::dispatch",
                    Level::INFO,
                    "zeta.event.handle"
                )
                .is_disabled()
            );
            assert!(
                tracing::span!(target: "plugins_titles::lib", Level::DEBUG, "titles.debug")
                    .is_disabled()
            );
        });
    }
}
