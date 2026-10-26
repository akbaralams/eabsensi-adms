use std::process::ExitCode;

use anyhow::Context;
use eabsensi_adms::cli::{self, Command};
use eabsensi_adms::config::Config;
use eabsensi_adms::http::{self, Limits};
use eabsensi_adms::state::{AppState, Biometrics, IngestRules, QueueSettings};
use eabsensi_adms::{offline, rekey};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    // `.env` hanya untuk pengembangan lokal; di produksi nilai datang dari environment proses.
    dotenvy::dotenv().ok();

    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}\n\n{}", cli::USAGE);
            return ExitCode::from(1);
        }
    };

    match run(command).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("galat: {error:#}");
            ExitCode::from(1)
        }
    }
}

async fn run(command: Command) -> anyhow::Result<ExitCode> {
    match command {
        Command::Help => {
            println!("{}", cli::USAGE);
            Ok(ExitCode::SUCCESS)
        }
        Command::Version => {
            println!("eabsensi-adms {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        Command::Serve => serve().await.map(|()| ExitCode::SUCCESS),
        Command::CheckOffline {
            factor,
            min_seconds,
        } => {
            let pool = connect_now().await?;
            let devices = offline::find(&pool, factor, min_seconds)
                .await
                .context("gagal membaca tabel devices")?;
            print!("{}", offline::render(&devices));
            Ok(if devices.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            })
        }
        Command::RekeyBiometrics { from, dry_run } => {
            let config = Config::from_env().context("konfigurasi tidak valid")?;
            let vault = config
                .vault
                .as_ref()
                .context("BIOMETRIC_KEY dan BIOMETRIC_KEY_ID wajib diisi untuk rekey-biometrics")?;
            let pool = connect_now().await?;
            let report = rekey::run(&pool, vault, from.as_deref(), dry_run)
                .await
                .context("rekey gagal di tengah jalan; aman dijalankan ulang")?;
            print!(
                "{}",
                rekey::render(&report, dry_run, vault.current_key_id())
            );
            Ok(if report.failed.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            })
        }
    }
}

/// Untuk perintah operasional: terhubung sekarang (galat langsung terlihat), kolam kecil.
async fn connect_now() -> anyhow::Result<PgPool> {
    let config = Config::from_env().context("konfigurasi tidak valid")?;
    PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(config.db_acquire_timeout)
        .connect(&config.database_url)
        .await
        .context("tidak bisa terhubung ke database (periksa DATABASE_URL)")
}

async fn serve() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env().context("konfigurasi tidak valid")?;
    tracing::info!(?config, "konfigurasi dimuat");
    if config.vault.is_none() {
        tracing::warn!(
            "BIOMETRIC_KEY tidak diisi: template biometrik tidak diterima dan tidak dikirim"
        );
    }

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
            ingest: IngestRules::from(&config),
            biometrics: Biometrics::from(&config),
        },
        Limits::from(&config),
    );

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("gagal membuka {}", config.bind_addr))?;
    tracing::info!(addr = %config.bind_addr, versi = env!("CARGO_PKG_VERSION"), "eabsensi-adms melayani");

    http::serve(listener, app, shutdown_signal())
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
