//! Uji HTTP: lapisan router, batas body, dan timeout. `readyz` sukses butuh `DATABASE_URL` (CI dan
//! `.env` lokal menyediakannya; skema harus sudah dimuat dari dump yang dipin `SCHEMA_VERSION`).

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use eabsensi_adms::http::{self, Limits};
use eabsensi_adms::state::AppState;
use http_body_util::BodyExt;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

fn limits() -> Limits {
    Limits {
        max_body_bytes: 1024,
        request_timeout: Duration::from_millis(200),
    }
}

async fn get(app: Router, path: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn unreachable_state() -> AppState {
    // Port 1 tidak melayani apa pun; pool malas tidak mencoba terhubung sampai dipakai.
    let db = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(100))
        .connect_lazy("postgres://u:p@127.0.0.1:1/x")
        .unwrap();
    AppState { db }
}

#[tokio::test]
async fn healthz_menjawab_tanpa_database() {
    let app = http::router(unreachable_state(), limits());

    let (status, body) = get(app, "/healthz").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn readyz_503_bila_database_tidak_terjangkau() {
    let app = http::router(unreachable_state(), limits());

    let (status, body) = get(app, "/readyz").await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "unavailable");
}

#[tokio::test]
async fn readyz_200_dengan_database_dan_skema_mesin() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL wajib diisi untuk uji ini");
    let db = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    let app = http::router(AppState { db }, limits());

    let (status, body) = get(app, "/readyz").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
}

#[tokio::test]
async fn rute_tak_dikenal_404() {
    let app = http::router(unreachable_state(), limits());

    let response = app
        .oneshot(
            Request::get("/iclock/tidak-ada")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

fn echo_router() -> Router {
    async fn echo(body: String) -> String {
        body
    }
    async fn lambat() -> &'static str {
        tokio::time::sleep(Duration::from_secs(2)).await;
        "terlambat"
    }
    http::apply_limits(
        Router::new()
            .route("/echo", post(echo))
            .route("/lambat", post(lambat)),
        limits(),
    )
}

#[tokio::test]
async fn body_di_bawah_batas_diterima() {
    let response = echo_router()
        .oneshot(
            Request::post("/echo")
                .body(Body::from("a".repeat(1024)))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn body_melebihi_batas_ditolak_413() {
    let response = echo_router()
        .oneshot(
            Request::post("/echo")
                .body(Body::from("a".repeat(1025)))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn request_yang_terlalu_lama_dihentikan_408() {
    let response = echo_router()
        .oneshot(Request::post("/lambat").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
}
