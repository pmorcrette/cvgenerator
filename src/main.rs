use anyhow::Context;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use cvgenerator::config::{Cli, Command};
use cvgenerator::storage::Store;
use cvgenerator::web::{AppState, example_resume, router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Logs go to stderr: stdout carries the MCP protocol in stdio mode.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let config = cli.config;
    let store = Store::open(&config.data_dir)
        .with_context(|| format!("cannot open data directory {}", config.data_dir.display()))?;
    if config.seed_example && store.list_cvs()?.is_empty() {
        let doc = store.create_cv(Some("jane-doe".into()), example_resume(), None)?;
        tracing::info!("created example CV '{}'", doc.id);
    }
    let state = AppState::new(store, config.clone());

    match cli.command.unwrap_or(Command::Serve) {
        Command::Mcp => cvgenerator::mcp::run_stdio(state).await,
        Command::Serve => {
            if config.api_token().is_none() {
                tracing::warn!(
                    "CV_API_TOKEN is not set: the admin UI, REST API and MCP endpoint are open to anyone"
                );
            }
            let listener = tokio::net::TcpListener::bind(config.bind)
                .await
                .with_context(|| format!("cannot listen on {}", config.bind))?;
            tracing::info!("listening on http://{}", config.bind);
            axum::serve(listener, router(state))
                .with_graceful_shutdown(shutdown_signal())
                .await?;
            Ok(())
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}
