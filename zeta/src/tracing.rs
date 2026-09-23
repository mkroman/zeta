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
use opentelemetry_semantic_conventions::{SCHEMA_URL, resource::SERVICE_VERSION};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config;

/// Returns a list of resource detectors to use to enrich OpenTelemetry attributes.
fn otel_resource_detectors() -> Vec<Box<dyn ResourceDetector>> {
    vec![
        Box::new(EnvResourceDetector::default()),
        Box::new(OsResourceDetector),
        Box::new(HostResourceDetector::default()),
        Box::new(K8sResourceDetector),
    ]
}

pub fn try_init(tracing: &config::TracingConfig) -> miette::Result<()> {
    // Create a tracing layer with the configured tracer
    let telemetry_layer = if tracing.enabled {
        // Set up the OTLP exporter
        let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .build()
            .into_diagnostic()
            .wrap_err("building otlp http exporter failed")?;
        // Set up resource detectors to enrich otel attributes
        let res_detectors = otel_resource_detectors();
        // Resource detectors for tracing context. The processor spawns its batching task on the
        // tokio runtime, so the async exporter can be driven from it.
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_span_processor(BatchSpanProcessor::builder(otlp_exporter, Tokio).build())
            .with_resource(
                Resource::builder_empty()
                    // `service.name` and `service.version` are the resource attributes every
                    // consumer groups by: https://opentelemetry.io/docs/specs/semconv/registry/attributes/service/
                    .with_service_name(env!("CARGO_PKG_NAME"))
                    .with_attribute(opentelemetry::KeyValue::new(
                        SERVICE_VERSION,
                        env!("CARGO_PKG_VERSION"),
                    ))
                    .with_detectors(&res_detectors)
                    // The schema URL identifies the semantic-convention version the resource
                    // follows, and must be a retrievable schema file:
                    // https://opentelemetry.io/docs/specs/otel/schemas/#schema-url
                    .with_schema_url(None::<opentelemetry::KeyValue>, SCHEMA_URL)
                    .build(),
            )
            .build();
        let scope = InstrumentationScope::builder(env!("CARGO_PKG_NAME"))
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_schema_url(SCHEMA_URL)
            .build();
        let tracer = provider.tracer_with_scope(scope);
        let layer = tracing_opentelemetry::layer().with_tracer(tracer);

        Some(layer)
    } else {
        None
    };

    let stdout_layer = tracing_subscriber::fmt::layer().json();

    // initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zeta=debug,reddit=debug,dendanskeordbog=debug".into()),
        )
        .with(telemetry_layer)
        .with(stdout_layer)
        .try_init()
        .into_diagnostic()
        .wrap_err("could not init registry")?;

    info!("tracing initialized");

    Ok(())
}
