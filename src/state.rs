use std::time::Duration;

use sqlx::PgPool;

use crate::config::Config;

/// Aturan antrean perintah ke mesin.
#[derive(Debug, Clone, Copy)]
pub struct QueueSettings {
    /// Perintah `sent` tanpa jawaban selama ini kembali `pending`.
    pub ack_timeout: Duration,
    /// Setelah sekian kali dikirim tanpa jawaban, perintah `failed`.
    pub max_attempts: i16,
}

impl From<&Config> for QueueSettings {
    fn from(config: &Config) -> Self {
        Self {
            ack_timeout: config.command_ack_timeout,
            max_attempts: config.command_max_attempts,
        }
    }
}

/// Dipakai bersama oleh semua handler. Murah di-clone (pool dibagi lewat `Arc` di dalam sqlx).
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub queue: QueueSettings,
}
