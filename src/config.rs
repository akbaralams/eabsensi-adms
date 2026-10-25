//! Konfigurasi service. Hanya dari environment (lihat `.env.example`).

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDateTime;
use chrono_tz::Tz;
use zeroize::Zeroizing;

use crate::vault::{self, Vault};

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8090";
const DEFAULT_MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_DB_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_DB_ACQUIRE_TIMEOUT_SECS: u64 = 5;
const DEFAULT_COMMAND_ACK_TIMEOUT_SECS: u64 = 120;
const DEFAULT_COMMAND_MAX_ATTEMPTS: i16 = 3;
const DEFAULT_ATTENDANCE_TIMEZONE: &str = "Asia/Jakarta";
const DEFAULT_MIN_LOG_TIME: &str = "2020-01-01 00:00:00";
const DEFAULT_SUSPECT_FUTURE_SECS: i64 = 500;
const LOG_TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("variabel environment {0} wajib diisi")]
    Missing(&'static str),
    #[error("variabel environment {name} tidak valid: {reason}")]
    Invalid { name: &'static str, reason: String },
}

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    pub max_body_bytes: usize,
    pub request_timeout: Duration,
    pub db_max_connections: u32,
    pub db_acquire_timeout: Duration,
    /// Perintah `sent` yang tidak dijawab mesin selama ini kembali `pending`.
    pub command_ack_timeout: Duration,
    /// Setelah sekian kali dikirim tanpa jawaban, perintah ditandai `failed`.
    pub command_max_attempts: i16,
    /// Zona bisnis penentu bulan sebuah punch (sama dengan `attendance.timezone` di eabsensi-v2).
    pub attendance_timezone: Tz,
    /// Waktu lokal mesin sebelum batas ini tidak dijadikan punch (`out_of_range`).
    pub min_log_time: NaiveDateTime,
    /// Log yang waktunya lebih dari sekian detik di depan waktu terima ditandai `is_suspect`.
    pub suspect_future: Duration,
    /// Brankas enkripsi biometrik; `None` bila `BIOMETRIC_KEY` tidak diisi (biometrik dinonaktifkan).
    pub vault: Option<Arc<Vault>>,
    /// Simpan juga foto wajah (`BIOPHOTO`), bukan hanya template. Bawaan mati.
    pub store_face_photo: bool,
}

// `database_url` memuat kata sandi, jadi tidak boleh ikut tercetak.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &"<disembunyikan>")
            .field("bind_addr", &self.bind_addr)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("request_timeout", &self.request_timeout)
            .field("db_max_connections", &self.db_max_connections)
            .field("db_acquire_timeout", &self.db_acquire_timeout)
            .field("command_ack_timeout", &self.command_ack_timeout)
            .field("command_max_attempts", &self.command_max_attempts)
            .field("attendance_timezone", &self.attendance_timezone)
            .field("min_log_time", &self.min_log_time)
            .field("suspect_future", &self.suspect_future)
            .field("vault", &self.vault)
            .field("store_face_photo", &self.store_face_photo)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Dipisah dari `from_env` agar bisa diuji tanpa menyentuh environment proses.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |name: &'static str| lookup(name).filter(|value| !value.trim().is_empty());

        let database_url = get("DATABASE_URL").ok_or(ConfigError::Missing("DATABASE_URL"))?;

        Ok(Self {
            database_url,
            bind_addr: parse(&get, "ADMS_BIND_ADDR", DEFAULT_BIND_ADDR)?,
            max_body_bytes: parse_positive(&get, "ADMS_MAX_BODY_BYTES", DEFAULT_MAX_BODY_BYTES)?,
            request_timeout: Duration::from_secs(parse_positive(
                &get,
                "ADMS_REQUEST_TIMEOUT_SECS",
                DEFAULT_REQUEST_TIMEOUT_SECS,
            )?),
            db_max_connections: parse_positive(
                &get,
                "ADMS_DB_MAX_CONNECTIONS",
                DEFAULT_DB_MAX_CONNECTIONS,
            )?,
            db_acquire_timeout: Duration::from_secs(parse_positive(
                &get,
                "ADMS_DB_ACQUIRE_TIMEOUT_SECS",
                DEFAULT_DB_ACQUIRE_TIMEOUT_SECS,
            )?),
            command_ack_timeout: Duration::from_secs(parse_positive(
                &get,
                "ADMS_COMMAND_ACK_TIMEOUT_SECS",
                DEFAULT_COMMAND_ACK_TIMEOUT_SECS,
            )?),
            command_max_attempts: parse_positive(
                &get,
                "ADMS_COMMAND_MAX_ATTEMPTS",
                DEFAULT_COMMAND_MAX_ATTEMPTS,
            )?,
            attendance_timezone: parse(
                &get,
                "ADMS_ATTENDANCE_TIMEZONE",
                DEFAULT_ATTENDANCE_TIMEZONE,
            )?,
            min_log_time: parse_log_time(&get, "ADMS_MIN_LOG_TIME", DEFAULT_MIN_LOG_TIME)?,
            suspect_future: Duration::from_secs(parse_positive(
                &get,
                "ADMS_SUSPECT_FUTURE_SECS",
                DEFAULT_SUSPECT_FUTURE_SECS,
            )? as u64),
            vault: build_vault(&get)?,
            store_face_photo: parse_bool(&get, "BIOMETRIC_STORE_FACE_PHOTO", false)?,
        })
    }
}

/// Brankas dari `BIOMETRIC_KEY` + `BIOMETRIC_KEY_ID` (+ `BIOMETRIC_OLD_KEYS` berbentuk
/// `id=base64,id=base64` untuk membaca baris lama setelah rotasi). Pesan galat tidak pernah
/// memuat nilai kunci.
fn build_vault(
    get: &impl Fn(&'static str) -> Option<String>,
) -> Result<Option<Arc<Vault>>, ConfigError> {
    let invalid = |name: &'static str, error: vault::VaultError| ConfigError::Invalid {
        name,
        reason: error.to_string(),
    };

    let old_raw = get("BIOMETRIC_OLD_KEYS");
    let Some(key_raw) = get("BIOMETRIC_KEY") else {
        if old_raw.is_some() {
            return Err(ConfigError::Invalid {
                name: "BIOMETRIC_OLD_KEYS",
                reason: "BIOMETRIC_KEY wajib diisi bila ada kunci lama".to_owned(),
            });
        }
        return Ok(None);
    };

    let key_id = get("BIOMETRIC_KEY_ID").ok_or(ConfigError::Missing("BIOMETRIC_KEY_ID"))?;
    let key = vault::parse_key(&key_raw).map_err(|error| invalid("BIOMETRIC_KEY", error))?;

    let mut old: Vec<(String, Zeroizing<[u8; vault::KEY_LEN]>)> = Vec::new();
    for entry in old_raw.iter().flat_map(|raw| raw.split(',')) {
        let (id, encoded) = entry.split_once('=').ok_or_else(|| ConfigError::Invalid {
            name: "BIOMETRIC_OLD_KEYS",
            reason: "format `id=base64,id=base64`".to_owned(),
        })?;
        let parsed =
            vault::parse_key(encoded).map_err(|error| invalid("BIOMETRIC_OLD_KEYS", error))?;
        old.push((id.trim().to_owned(), parsed));
    }

    Vault::new((key_id.trim(), &key), &old)
        .map(|vault| Some(Arc::new(vault)))
        .map_err(|error| invalid("BIOMETRIC_KEY_ID", error))
}

fn parse_bool(
    get: &impl Fn(&'static str) -> Option<String>,
    name: &'static str,
    default: bool,
) -> Result<bool, ConfigError> {
    match get(name)
        .map(|raw| raw.trim().to_ascii_lowercase())
        .as_deref()
    {
        None => Ok(default),
        Some("true" | "1" | "yes" | "on") => Ok(true),
        Some("false" | "0" | "no" | "off") => Ok(false),
        Some(_) => Err(ConfigError::Invalid {
            name,
            reason: "harus true atau false".to_owned(),
        }),
    }
}

fn parse_log_time(
    get: &impl Fn(&'static str) -> Option<String>,
    name: &'static str,
    default: &str,
) -> Result<NaiveDateTime, ConfigError> {
    let raw = get(name).unwrap_or_else(|| default.to_owned());
    NaiveDateTime::parse_from_str(raw.trim(), LOG_TIME_FORMAT).map_err(|error| {
        ConfigError::Invalid {
            name,
            reason: format!("{error} (format YYYY-MM-DD HH:MM:SS)"),
        }
    })
}

fn parse<T>(
    get: &impl Fn(&'static str) -> Option<String>,
    name: &'static str,
    default: &str,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    let raw = get(name).unwrap_or_else(|| default.to_owned());
    raw.trim()
        .parse()
        .map_err(|error: T::Err| ConfigError::Invalid {
            name,
            reason: error.to_string(),
        })
}

fn parse_positive<T>(
    get: &impl Fn(&'static str) -> Option<String>,
    name: &'static str,
    default: T,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr + PartialOrd + Default + fmt::Display,
    T::Err: fmt::Display,
{
    let value: T = parse(get, name, &default.to_string())?;
    if value <= T::default() {
        return Err(ConfigError::Invalid {
            name,
            reason: "harus lebih besar dari 0".to_owned(),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn memakai_nilai_bawaan_bila_hanya_database_url_diisi() {
        let config = Config::from_lookup(lookup(&[("DATABASE_URL", "postgres://x/y")])).unwrap();

        assert_eq!(config.bind_addr, "0.0.0.0:8090".parse().unwrap());
        assert_eq!(config.max_body_bytes, 8 * 1024 * 1024);
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert_eq!(config.db_max_connections, 10);
        assert_eq!(config.command_ack_timeout, Duration::from_secs(120));
        assert_eq!(config.command_max_attempts, 3);
        assert_eq!(config.attendance_timezone, chrono_tz::Asia::Jakarta);
        assert_eq!(config.min_log_time.to_string(), "2020-01-01 00:00:00");
        assert_eq!(config.suspect_future, Duration::from_secs(500));
    }

    #[test]
    fn database_url_wajib() {
        let error = Config::from_lookup(lookup(&[])).unwrap_err();
        assert_eq!(error, ConfigError::Missing("DATABASE_URL"));

        let error = Config::from_lookup(lookup(&[("DATABASE_URL", "   ")])).unwrap_err();
        assert_eq!(error, ConfigError::Missing("DATABASE_URL"));
    }

    #[test]
    fn nilai_dari_environment_menimpa_bawaan() {
        let config = Config::from_lookup(lookup(&[
            ("DATABASE_URL", "postgres://x/y"),
            ("ADMS_BIND_ADDR", "127.0.0.1:9000"),
            ("ADMS_MAX_BODY_BYTES", "1024"),
            ("ADMS_REQUEST_TIMEOUT_SECS", "5"),
        ]))
        .unwrap();

        assert_eq!(config.bind_addr, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(config.max_body_bytes, 1024);
        assert_eq!(config.request_timeout, Duration::from_secs(5));
    }

    #[test]
    fn menolak_nilai_tidak_valid_dan_nol() {
        let base = ("DATABASE_URL", "postgres://x/y");

        let alamat = Config::from_lookup(lookup(&[base, ("ADMS_BIND_ADDR", "bukan-alamat")]));
        assert!(matches!(
            alamat,
            Err(ConfigError::Invalid {
                name: "ADMS_BIND_ADDR",
                ..
            })
        ));

        let nol = Config::from_lookup(lookup(&[base, ("ADMS_MAX_BODY_BYTES", "0")]));
        assert!(matches!(
            nol,
            Err(ConfigError::Invalid {
                name: "ADMS_MAX_BODY_BYTES",
                ..
            })
        ));
    }

    #[test]
    fn zona_dan_batas_waktu_log_divalidasi() {
        let base = ("DATABASE_URL", "postgres://x/y");

        let zona = Config::from_lookup(lookup(&[
            base,
            ("ADMS_ATTENDANCE_TIMEZONE", "Mars/Olympus"),
        ]));
        assert!(matches!(
            zona,
            Err(ConfigError::Invalid {
                name: "ADMS_ATTENDANCE_TIMEZONE",
                ..
            })
        ));

        let waktu = Config::from_lookup(lookup(&[base, ("ADMS_MIN_LOG_TIME", "kemarin")]));
        assert!(matches!(
            waktu,
            Err(ConfigError::Invalid {
                name: "ADMS_MIN_LOG_TIME",
                ..
            })
        ));

        let ok = Config::from_lookup(lookup(&[
            base,
            ("ADMS_ATTENDANCE_TIMEZONE", "Asia/Makassar"),
            ("ADMS_MIN_LOG_TIME", "2021-06-01 08:30:00"),
            ("ADMS_SUSPECT_FUTURE_SECS", "60"),
        ]))
        .unwrap();
        assert_eq!(ok.attendance_timezone, chrono_tz::Asia::Makassar);
        assert_eq!(ok.min_log_time.to_string(), "2021-06-01 08:30:00");
        assert_eq!(ok.suspect_future, Duration::from_secs(60));
    }

    #[test]
    fn biometrik_nonaktif_tanpa_kunci_dan_foto_wajah_mati_secara_bawaan() {
        let config = Config::from_lookup(lookup(&[("DATABASE_URL", "postgres://x/y")])).unwrap();

        assert!(config.vault.is_none());
        assert!(!config.store_face_photo);
    }

    #[test]
    fn kunci_biometrik_membentuk_brankas_dan_tidak_tercetak() {
        use base64::Engine;
        let key = base64::engine::general_purpose::STANDARD.encode([5u8; 32]);
        let old = base64::engine::general_purpose::STANDARD.encode([6u8; 32]);
        let config = Config::from_lookup(lookup(&[
            ("DATABASE_URL", "postgres://x/y"),
            ("BIOMETRIC_KEY", &key),
            ("BIOMETRIC_KEY_ID", "k2"),
            ("BIOMETRIC_OLD_KEYS", &format!("k1={old}")),
            ("BIOMETRIC_STORE_FACE_PHOTO", "TRUE"),
        ]))
        .unwrap();

        assert_eq!(config.vault.as_ref().unwrap().current_key_id(), "k2");
        assert!(config.store_face_photo);
        let printed = format!("{config:?}");
        assert!(!printed.contains(&key) && !printed.contains(&old));
    }

    #[test]
    fn konfigurasi_biometrik_yang_salah_ditolak_tanpa_membocorkan_kunci() {
        use base64::Engine;
        let key = base64::engine::general_purpose::STANDARD.encode([5u8; 32]);
        let base = ("DATABASE_URL", "postgres://x/y");

        let no_id = Config::from_lookup(lookup(&[base, ("BIOMETRIC_KEY", &key)]));
        assert_eq!(no_id.unwrap_err(), ConfigError::Missing("BIOMETRIC_KEY_ID"));

        let short = Config::from_lookup(lookup(&[
            base,
            ("BIOMETRIC_KEY", "pendek"),
            ("BIOMETRIC_KEY_ID", "k1"),
        ]))
        .unwrap_err();
        assert!(matches!(
            short,
            ConfigError::Invalid {
                name: "BIOMETRIC_KEY",
                ..
            }
        ));
        assert!(!short.to_string().contains("pendek"));

        let bad_id = Config::from_lookup(lookup(&[
            base,
            ("BIOMETRIC_KEY", &key),
            ("BIOMETRIC_KEY_ID", "id salah"),
        ]));
        assert!(matches!(
            bad_id,
            Err(ConfigError::Invalid {
                name: "BIOMETRIC_KEY_ID",
                ..
            })
        ));

        let orphan_old = Config::from_lookup(lookup(&[base, ("BIOMETRIC_OLD_KEYS", "k1=abc")]));
        assert!(matches!(
            orphan_old,
            Err(ConfigError::Invalid {
                name: "BIOMETRIC_OLD_KEYS",
                ..
            })
        ));

        let bad_old = Config::from_lookup(lookup(&[
            base,
            ("BIOMETRIC_KEY", &key),
            ("BIOMETRIC_KEY_ID", "k2"),
            ("BIOMETRIC_OLD_KEYS", "tanpa-sama-dengan"),
        ]));
        assert!(matches!(
            bad_old,
            Err(ConfigError::Invalid {
                name: "BIOMETRIC_OLD_KEYS",
                ..
            })
        ));

        let flag = Config::from_lookup(lookup(&[base, ("BIOMETRIC_STORE_FACE_PHOTO", "mungkin")]));
        assert!(matches!(
            flag,
            Err(ConfigError::Invalid {
                name: "BIOMETRIC_STORE_FACE_PHOTO",
                ..
            })
        ));
    }

    #[test]
    fn debug_tidak_membocorkan_database_url() {
        let config =
            Config::from_lookup(lookup(&[("DATABASE_URL", "postgres://user:rahasia@h/db")]))
                .unwrap();

        let tercetak = format!("{config:?}");

        assert!(!tercetak.contains("rahasia"));
    }
}
