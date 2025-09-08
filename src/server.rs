use crate::compute::{ComputeApi, ComputeClient};
use crate::github::{GithubApi, GithubClient};
use crate::metadata::get_gcp_environment;
use crate::telemetry::PropagateHeaders;
use crate::webhook::handle_workflow_job_event;
use axum::Router;
use axum::body::Body;
use axum::extract::FromRef;
use axum::http::Request;
use axum::routing::{get, post};
use axum_github_webhook_extract::GithubToken;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::{info, instrument};

/// Application state containing shared resources
#[derive(Clone)]
pub struct AppState {
    pub compute_client: std::sync::Arc<dyn ComputeApi>,
    pub github_client: std::sync::Arc<dyn GithubApi>,
    pub project_id: Arc<String>,
    pub region: Arc<String>,
    pub secret: GithubToken,
    pub token: Arc<String>,
}

#[derive(Debug, Deserialize)]
struct GithubCredentialsSecret {
    token: String,
    secret: String,
}

impl AppState {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let creds = std::env::var("GITHUB_CREDENTIALS")?;
        let creds: GithubCredentialsSecret = serde_json::from_str(&creds)?;

        let (environment_result, compute_result) =
            tokio::join!(get_gcp_environment(), ComputeClient::new());

        let (project_id, region) =
            environment_result.map_err(|e| -> Box<dyn std::error::Error> { e })?;
        let compute_client = compute_result?;

        Ok(Self {
            compute_client: Arc::new(compute_client),
            github_client: Arc::new(GithubClient::new()),
            project_id: Arc::new(project_id),
            region: Arc::new(region),
            secret: GithubToken(Arc::new(creds.secret)),
            token: Arc::new(creds.token),
        })
    }
}

impl FromRef<AppState> for GithubToken {
    fn from_ref(input: &AppState) -> Self {
        input.secret.clone()
    }
}

/// Simple ping endpoint for health checks
#[instrument]
pub async fn ping() -> &'static str {
    "pong"
}

/// Health check endpoint that returns service status and metadata
#[instrument]
pub async fn health_check(request: Request<Body>) -> String {
    info!(
        uri = %request.uri(),
        method = %request.method(),
    );

    json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "headers": request.headers().iter().map(|(k, v)| (k.as_str(), v.to_str().unwrap_or("invalid utf8"))).collect::<HashMap<_, _>>(),
    }).to_string()
}

/// Creates the Axum router with all routes and middleware configured
pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/webhook", post(handle_workflow_job_event).with_state(state))
        .route("/ping", get(ping))
        .route("/health_check", post(health_check))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http().make_span_with(PropagateHeaders)),
        )
}

/// Graceful shutdown signal handler
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("signal received, starting graceful shutdown");
}
