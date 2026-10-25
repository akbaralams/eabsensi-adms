//! Lapisan HTTP. Parser protokol sengaja dipisah dari sini (dikerjakan di task berikutnya).

mod health;
mod iclock;

use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{get, post};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::state::AppState;

/// Batas yang berlaku untuk setiap request mesin.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_body_bytes: usize,
    pub request_timeout: Duration,
}

impl From<&Config> for Limits {
    fn from(config: &Config) -> Self {
        Self {
            max_body_bytes: config.max_body_bytes,
            request_timeout: config.request_timeout,
        }
    }
}

/// Router aplikasi lengkap dengan batas ukuran body, timeout, dan pelacakan request.
pub fn router(state: AppState, limits: Limits) -> Router {
    let routes = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/iclock/cdata", get(iclock::connect))
        .route("/iclock/getrequest", get(iclock::poll))
        .route("/iclock/devicecmd", post(iclock::command_result))
        .with_state(state);

    apply_limits(routes, limits)
}

/// Dipisah agar batas yang sama bisa dipasang (dan diuji) pada router lain.
pub fn apply_limits(router: Router, limits: Limits) -> Router {
    router
        .layer(DefaultBodyLimit::max(limits.max_body_bytes))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            limits.request_timeout,
        ))
        .layer(TraceLayer::new_for_http())
}
