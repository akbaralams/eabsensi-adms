use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
pub struct Health {
    status: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
pub struct Readiness {
    status: &'static str,
}

/// Liveness: proses hidup dan bisa menjawab. Tidak menyentuh database.
pub async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Readiness: database terjangkau dan tabel mesin (skema v2) ada.
pub async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<Readiness>) {
    let probe = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM devices) AS "has_rows!""#)
        .fetch_one(&state.db)
        .await;

    match probe {
        Ok(_) => (StatusCode::OK, Json(Readiness { status: "ready" })),
        Err(error) => {
            tracing::warn!(%error, "pemeriksaan kesiapan database gagal");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(Readiness {
                    status: "unavailable",
                }),
            )
        }
    }
}
