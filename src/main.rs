use rustpulse::{Config, SharedState, build_app};
use tracing::info;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rustpulse=info,tower_http=info".into()),
        )
        .init();

    let config = Config::from_env();
    let state = SharedState::new(config.clone());
    state.spawn_housekeeping();

    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .expect("failed to bind TCP listener");

    info!("RustPulse backend listening on {}", config.bind_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server exited unexpectedly");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install terminate handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
