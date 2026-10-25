use anyhow::Context;
use eabsensi_adms::config::Config;
use eabsensi_adms::http::{self, Limits};
use eabsensi_adms::state::{AppState, QueueSettings};
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `.env` hanya untuk pengembangan lokal; di produksi nilai datang dari environment proses.
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env().context("konfigurasi tidak valid")?;
    tracing::info!(?config, "konfigurasi dimuat");

    // Pool dibuat malas: service tetap hidup (dan `/healthz` menjawab) walau database sedang
    // tidak terjangkau; `/readyz` yang melaporkannya.
    let db = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .acquire_timeout(config.db_acquire_timeout)
        .connect_lazy(&config.database_url)
        .context("DATABASE_URL tidak valid")?;

    let app = http::router(
        AppState {
            db,
            queue: QueueSettings::from(&config),
        },
        Limits::from(&config),
    );

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("gagal membuka {}", config.bind_addr))?;
    tracing::info!(addr = %config.bind_addr, "eabsensi-adms melayani");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server berhenti dengan galat")?;

    tracing::info!("eabsensi-adms berhenti");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "gagal memasang penangan Ctrl+C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "gagal memasang penangan SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
