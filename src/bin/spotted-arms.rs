use std::net::{IpAddr, Ipv6Addr};
use clap::Parser;
use tokio::net::TcpListener;
use tracing::info;

#[derive(Debug, Parser)]
#[command(
    name = "spotted-arms",
    version,
    about = "Spotted Arms",
    long_about = "Spotted Arms — an ephemeral GitHub Actions runner on Google Compute Engine"
)]
struct Cli {
    /// 🚪 TCP port for the HTTP server
    #[arg(long, short = 'p', env = "PORT", default_value_t = 3000)]
    port: u16,

    /// 🔑 GitHub credentials JSON: {"token":"...","secret":"..."}
    #[arg(long, env = "GITHUB_CREDENTIALS")]
    github_credentials: Option<String>,

    /// 🧩 GCE region instance template name
    #[arg(long, env = "INSTANCE_TEMPLATE")]
    instance_template: Option<String>,

    /// 🏷️ Google Cloud project ID (sets GOOGLE_CLOUD_PROJECT and GCP_PROJECT)
    #[arg(long = "project-id", env = "GOOGLE_CLOUD_PROJECT")]
    project_id: Option<String>,

    /// 📍 Google Cloud zone (e.g., us-central1-f)
    #[arg(long = "zone", env = "GOOGLE_CLOUD_ZONE")]
    zone: Option<String>,

    /// 📊 Cloud Trace project override for telemetry
    #[arg(long = "telemetry-project-id", env = "PROJECT_ID")]
    telemetry_project_id: Option<String>,

    /// 🔔 Webhook endpoint path (default: /webhook)
    #[arg(long = "webhook-path", env = "WEBHOOK_PATH", default_value = "/webhook")]
    webhook_path: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pid1::relaunch_if_pid1()?;

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Parse CLI (supports environment via clap's env feature)
    let cli = Cli::parse();

    // Resolve project/region using CLI values when provided; otherwise discover
    let (project_id, region) = if cli.project_id.is_some() || cli.zone.is_some() {
        let discovered = if cli.project_id.is_none() || cli.zone.is_none() {
            Some(spotted_arms::server::AppState::discover_project_region().await?)
        } else {
            None
        };
        let project_id = cli
            .project_id
            .clone()
            .or_else(|| discovered.as_ref().map(|(p, _)| p.clone()))
            .expect("project id resolution");
        let region = if let Some(zone) = &cli.zone {
            zone
                .rsplit_once('-')
                .map(|(r, _)| r.to_string())
                .unwrap_or_else(|| zone.clone())
        } else {
            discovered.unwrap().1
        };
        (project_id, region)
    } else {
        spotted_arms::server::AppState::discover_project_region().await?
    };

    // Initialize telemetry with optional override
    spotted_arms::telemetry::init_tracing(cli.telemetry_project_id.clone()).await?;

    // Build application state from CLI-sourced configuration
    let creds = cli
        .github_credentials
        .as_deref()
        .ok_or("Missing required --github-credentials or GITHUB_CREDENTIALS env")?;
    let instance_template = cli
        .instance_template
        .as_deref()
        .ok_or("Missing required --instance-template or INSTANCE_TEMPLATE env")?;

    let state = spotted_arms::server::AppState::new_with(
        creds,
        project_id,
        region,
        instance_template.to_string(),
    )
    .await?;

    // Normalize webhook path and build app
    let mut webhook_path = cli.webhook_path.clone();
    if webhook_path.is_empty() || webhook_path == "/" {
        info!("Invalid WEBHOOK_PATH '{}'; defaulting to /webhook", webhook_path);
        webhook_path = "/webhook".to_string();
    }
    if !webhook_path.starts_with('/') {
        webhook_path = format!("/{}", webhook_path);
    }
    let app = spotted_arms::server::create_app(state, &webhook_path);

    // Determine port
    // Note: if neither PORT env nor --port is passed, default is 3000
    let port = cli.port;
    if std::env::var("PORT").is_err()
        && !std::env::args().any(|a| a == "--port" || a.starts_with("--port=") || a == "-p")
        && port == 3000
    {
        info!("PORT not set; defaulting to 3000");
    }

    let listener = TcpListener::bind((IpAddr::from(Ipv6Addr::UNSPECIFIED), port))
        .await
        .unwrap();

    info!("Starting server on port {}", port);

    // Enable HTTP/2 with axum::serve
    axum::serve(listener, app)
        .with_graceful_shutdown(spotted_arms::server::shutdown_signal())
        .await?;

    Ok(())
}
