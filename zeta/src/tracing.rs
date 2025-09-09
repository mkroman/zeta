use std::env;

use miette::{IntoDiagnostic, WrapErr};
use opentelemetry::InstrumentationScope;
use opentelemetry::trace::TracerProvider;
use opentelemetry_resource_detectors::{
    HostResourceDetector, K8sResourceDetector, OsResourceDetector,
};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::resource::{EnvResourceDetector, ResourceDetector};
use tracing::{debug, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config;

/// Returns a list of resource detectors to use to enrich OTel attributes.
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
        // Resource detectors for tracing context
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(otlp_exporter)
            .with_resource(
                Resource::builder_empty()
                    .with_service_name(env!("CARGO_PKG_NAME"))
                    .with_detectors(&res_detectors)
                    .build(),
            )
            .build();
        let scope = InstrumentationScope::builder(env!("CARGO_PKG_NAME"))
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_schema_url("https://opentelemetry.io/schema/1.0.0")
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
                .unwrap_or_else(|_| "zeta=debug".into()),
        )
        .with(telemetry_layer)
        .with(stdout_layer)
        .try_init()
        .into_diagnostic()
        .wrap_err("could not init registry")?;

    info!("tracing initialized");

    Ok(())
}
