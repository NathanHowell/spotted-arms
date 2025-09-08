use std::net::{IpAddr, Ipv6Addr};
use std::str::FromStr;
use tokio::net::TcpListener;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pid1::relaunch_if_pid1()?;

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    ephemeral_runner::telemetry::init_tracing().await?;

    let state = ephemeral_runner::server::AppState::new().await?;

    let app = ephemeral_runner::server::create_app(state);

    // Require PORT to be provided via environment variable; do not default.
    let port_str = std::env::var("PORT")?;
    let port = u16::from_str(&port_str)?;

    let listener = TcpListener::bind((IpAddr::from(Ipv6Addr::UNSPECIFIED), port))
        .await
        .unwrap();

    info!("Starting server on port {}", port);

    // Enable HTTP/2 with axum::serve
    axum::serve(listener, app)
        .with_graceful_shutdown(ephemeral_runner::server::shutdown_signal())
        .await?;

    Ok(())
}
