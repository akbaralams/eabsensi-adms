//! Konfigurasi service. Hanya dari environment (lihat `.env.example`).

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use chrono::NaiveDateTime;
use chrono_tz::Tz;

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
        })
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
    fn debug_tidak_membocorkan_database_url() {
        let config =
            Config::from_lookup(lookup(&[("DATABASE_URL", "postgres://user:rahasia@h/db")]))
                .unwrap();

        let tercetak = format!("{config:?}");

        assert!(!tercetak.contains("rahasia"));
    }
}
