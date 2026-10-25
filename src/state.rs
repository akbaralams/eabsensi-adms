use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDateTime;
use chrono_tz::Tz;

use sqlx::PgPool;

use crate::config::Config;
use crate::vault::Vault;

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

/// Aturan penerimaan log absensi (`docs/schema-proposal.md` bagian 4 dan keputusan A3/A4).
#[derive(Debug, Clone, Copy)]
pub struct IngestRules {
    /// Zona bisnis penentu bulan punch untuk pemeriksaan periode terkunci.
    pub attendance_zone: Tz,
    /// Waktu lokal mesin sebelum batas ini menjadi `out_of_range`.
    pub min_device_time: NaiveDateTime,
    /// Waktu mesin lebih dari ini di depan waktu terima ditandai `is_suspect`.
    pub suspect_after: Duration,
}

impl From<&Config> for IngestRules {
    fn from(config: &Config) -> Self {
        Self {
            attendance_zone: config.attendance_timezone,
            min_device_time: config.min_log_time,
            suspect_after: config.suspect_future,
        }
    }
}

/// Pengaturan biometrik (D1). Tanpa brankas, template tidak diterima dan tidak bisa dikirim.
#[derive(Debug, Clone)]
pub struct Biometrics {
    pub vault: Option<Arc<Vault>>,
    /// Simpan juga foto wajah (`BIOPHOTO`).
    pub store_face_photo: bool,
}

impl Biometrics {
    pub fn disabled() -> Self {
        Self {
            vault: None,
            store_face_photo: false,
        }
    }
}

impl From<&Config> for Biometrics {
    fn from(config: &Config) -> Self {
        Self {
            vault: config.vault.clone(),
            store_face_photo: config.store_face_photo,
        }
    }
}

/// Dipakai bersama oleh semua handler. Murah di-clone (pool dibagi lewat `Arc` di dalam sqlx).
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub queue: QueueSettings,
    pub ingest: IngestRules,
    pub biometrics: Biometrics,
}
