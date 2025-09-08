use axum::http::Request;
use opentelemetry::global;
use opentelemetry::propagation::TextMapPropagator;
use opentelemetry::trace::TracerProvider;
use opentelemetry_gcloud_trace::GcpCloudTraceExporterBuilder;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::TracerProviderBuilder;
use std::collections::HashMap;
use tower_http::trace::MakeSpan;
use tracing::info_span;
use tracing_opentelemetry::{OpenTelemetryLayer, OpenTelemetrySpanExt};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize OpenTelemetry with Google Cloud Trace
pub async fn init_tracing(project_id_override: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    // Prefer explicit override, otherwise discover via metadata
    let project_id = if let Some(pid) = project_id_override {
        pid
    } else {
        let (pid, _region) = crate::metadata::get_gcp_environment()
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { e })?;
        pid
    };

    // Create Google Cloud Trace exporter
    let gcp_trace_exporter = GcpCloudTraceExporterBuilder::new(project_id);
    let tracer_provider = gcp_trace_exporter
        .create_provider_from_builder(
            TracerProviderBuilder::default().with_resource(
                Resource::builder()
                    .with_attributes(vec![
                        opentelemetry::KeyValue::new("service.name", "spotless-arms"),
                        opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                    ])
                    .build(),
            ),
        )
        .await?;

    // Set global tracer provider
    global::set_text_map_propagator(TraceContextPropagator::new());
    global::set_tracer_provider(tracer_provider.clone());

    // Create OpenTelemetry layer
    let tracer = tracer_provider.tracer("spotless-arms");
    let telemetry_layer = OpenTelemetryLayer::new(tracer);

    // Initialize tracing subscriber with both console and OpenTelemetry layers
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer().json())
        .with(telemetry_layer)
        .init();

    Ok(())
}

/// Custom trace span creator that propagates OpenTelemetry context from HTTP headers
#[derive(Copy, Clone, Debug)]
pub struct PropagateHeaders;

impl<B> MakeSpan<B> for PropagateHeaders {
    fn make_span(&mut self, request: &Request<B>) -> tracing::Span {
        static TRACEPARENT: &str = "traceparent";

        let traceparent = request
            .headers()
            .get(TRACEPARENT)
            .and_then(|v| v.to_str().ok());

        let extractor = if let Some(tp) = traceparent {
            HashMap::from([(TRACEPARENT.to_string(), tp.to_string())])
        } else {
            HashMap::new()
        };

        let propagator = TraceContextPropagator::new();
        let context = propagator.extract(&extractor);

        let span = info_span!("axum");
        span.set_parent(context);
        span
    }
}
