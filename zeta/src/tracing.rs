use miette::IntoDiagnostic;
use opentelemetry::{
    trace::{TraceError, TracerProvider},
    KeyValue,
};
use opentelemetry_sdk::export::trace::SpanExporter;
use opentelemetry_sdk::trace::TracerProvider as SdkTracerProvider;
use opentelemetry_sdk::{runtime, Resource};
use opentelemetry_semantic_conventions::{
    resource::{SERVICE_NAME, SERVICE_VERSION},
    SCHEMA_URL,
};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::cli::Format;
use zeta::config;

// Create a Resource that captures information about the entity for which telemetry is recorded.
fn resource() -> Resource {
    Resource::from_schema_url(
        [
            KeyValue::new(SERVICE_NAME, env!("CARGO_PKG_NAME")),
            KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
        ],
        SCHEMA_URL,
    )
}

fn init_tracer_provider() -> Result<SdkTracerProvider, TraceError> {
    let mut exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()?;

    exporter.set_resource(&resource());

    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter, runtime::Tokio)
        .build())
}

pub fn init(stdout_format: &Format, _tracing: &config::TracingConfig) -> miette::Result<()> {
    // Create a tracing layer with the configured tracer
    let tracer_provider = init_tracer_provider().into_diagnostic()?;

    // Create a tracing layer with the configured tracer
    let tracer = tracer_provider.tracer(env!("CARGO_PKG_NAME"));
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);

    // initialize tracing
    let base = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zeta=trace".into()),
        )
        .with(telemetry);

    match stdout_format {
        Format::Json => base.with(tracing_subscriber::fmt::layer().json()).init(),
        Format::Pretty => base.with(tracing_subscriber::fmt::layer().pretty()).init(),
        Format::Compact => base.with(tracing_subscriber::fmt::layer().compact()).init(),
    };

    info!("tracing initialized");

    Ok(())
}
