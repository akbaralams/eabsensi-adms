use sqlx::PgPool;

/// Dipakai bersama oleh semua handler. Murah di-clone (pool dibagi lewat `Arc` di dalam sqlx).
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
}
