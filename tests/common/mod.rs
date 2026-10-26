//! Bantuan uji integrasi: database bersama (`DATABASE_URL`, skema v2 sudah dimuat), mesin dan
//! pegawai uji dengan awalan `TEST-ADMS-`, dan pemanggilan router lewat `oneshot`.
//!
//! Role database dev tidak punya `CREATEDB`, jadi `#[sqlx::test]` (database sementara per uji)
//! tidak dipakai. Sebagai gantinya tiap uji membuat data bernama unik dan membersihkannya sendiri.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use eabsensi_adms::http::{self, Limits};
use eabsensi_adms::state::{AppState, Biometrics, IngestRules, QueueSettings};
use http_body_util::BodyExt;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

pub const USER_AGENT: &str = "iClock Proxy/1.09";

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}{}-{nanos}-{n}", std::process::id())
}

fn unique_number() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("9{:06}{:03}", nanos % 1_000_000, n % 1000)
}

pub async fn pool() -> PgPool {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL wajib diisi untuk uji integrasi");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("database tidak terjangkau");

    // Sisa uji yang gagal di tengah jalan; hanya yang sudah tua agar tidak mengganggu uji paralel.
    // `device_logs.device_id` bersifat RESTRICT, jadi log sisa uji dihapus lebih dulu.
    sqlx::query(
        "DELETE FROM device_logs WHERE device_id IN (SELECT id FROM devices \
          WHERE serial_number LIKE 'TEST-ADMS-%' \
            AND created_at < (now() AT TIME ZONE 'UTC') - interval '1 hour')",
    )
    .execute(&pool)
    .await
    .expect("pembersihan log sisa uji gagal");
    sqlx::query(
        "DELETE FROM devices WHERE serial_number LIKE 'TEST-ADMS-%' \
         AND created_at < (now() AT TIME ZONE 'UTC') - interval '1 hour'",
    )
    .execute(&pool)
    .await
    .expect("pembersihan sisa uji gagal");
    sqlx::query(
        "DELETE FROM employees WHERE department_id IN (SELECT id FROM departments           WHERE name = 'Departemen uji ADMS'             AND created_at < (now() AT TIME ZONE 'UTC') - interval '1 hour')",
    )
    .execute(&pool)
    .await
    .expect("pembersihan pegawai uji gagal");
    sqlx::query(
        "DELETE FROM departments WHERE name = 'Departemen uji ADMS'             AND created_at < (now() AT TIME ZONE 'UTC') - interval '1 hour'",
    )
    .execute(&pool)
    .await
    .expect("pembersihan departemen uji gagal");
    pool
}

pub fn app(pool: &PgPool, queue: QueueSettings) -> Router {
    app_with(pool, queue, Biometrics::disabled())
}

pub fn app_with(pool: &PgPool, queue: QueueSettings, biometrics: Biometrics) -> Router {
    http::router(
        AppState {
            db: pool.clone(),
            queue,
            ingest: default_ingest(),
            biometrics,
        },
        Limits {
            max_body_bytes: 1024 * 1024,
            request_timeout: Duration::from_secs(10),
        },
    )
}

pub fn default_ingest() -> IngestRules {
    IngestRules {
        attendance_zone: chrono_tz::Asia::Jakarta,
        min_device_time: chrono::NaiveDate::from_ymd_opt(2020, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        suspect_after: Duration::from_secs(500),
    }
}

pub fn default_queue() -> QueueSettings {
    QueueSettings {
        ack_timeout: Duration::from_secs(120),
        max_attempts: 3,
    }
}

pub struct Reply {
    pub status: StatusCode,
    pub content_type: String,
    pub body: String,
}

/// Request ke router dengan `User-Agent` mesin (kecuali `user_agent` diganti).
pub async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: &str,
    user_agent: Option<&str>,
) -> Reply {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(agent) = user_agent {
        builder = builder.header(header::USER_AGENT, agent);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        content_type,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

pub async fn get(app: &Router, uri: &str) -> Reply {
    send(app, Method::GET, uri, "", Some(USER_AGENT)).await
}

pub async fn post(app: &Router, uri: &str, body: &str) -> Reply {
    send(app, Method::POST, uri, body, Some(USER_AGENT)).await
}

/// Mesin uji di tabel `devices`.
pub struct TestDevice {
    pub id: i64,
    pub serial: String,
}

impl TestDevice {
    pub async fn create(pool: &PgPool) -> Self {
        Self::create_with(pool, true).await
    }

    pub async fn create_inactive(pool: &PgPool) -> Self {
        Self::create_with(pool, false).await
    }

    async fn create_with(pool: &PgPool, active: bool) -> Self {
        let serial = unique("TEST-ADMS-");
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO devices (serial_number, name, is_active, created_at, updated_at) \
             VALUES ($1, 'Mesin uji', $2, now() AT TIME ZONE 'UTC', now() AT TIME ZONE 'UTC') \
             RETURNING id",
        )
        .bind(&serial)
        .bind(active)
        .fetch_one(pool)
        .await
        .unwrap();
        Self { id, serial }
    }

    pub async fn queue(&self, pool: &PgPool, command_type: &str, priority: i16) -> i64 {
        self.queue_for(pool, command_type, priority, None).await
    }

    pub async fn queue_for(
        &self,
        pool: &PgPool,
        command_type: &str,
        priority: i16,
        employee_id: Option<i64>,
    ) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO device_commands \
                 (device_id, command_type, priority, status, employee_id, created_at, updated_at) \
             VALUES ($1, $2, $3, 'pending', $4, now() AT TIME ZONE 'UTC', now() AT TIME ZONE 'UTC') \
             RETURNING id",
        )
        .bind(self.id)
        .bind(command_type)
        .bind(priority)
        .bind(employee_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    pub async fn command_row(&self, pool: &PgPool, id: i64) -> CommandRow {
        let row: (String, i16, Option<i32>, Option<String>, bool) = sqlx::query_as(
            "SELECT status, attempts, return_code, result_message, completed_at IS NOT NULL \
               FROM device_commands WHERE id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
        CommandRow {
            status: row.0,
            attempts: row.1,
            return_code: row.2,
            result_message: row.3,
            completed: row.4,
        }
    }

    pub async fn count_commands(&self, pool: &PgPool, command_type: &str, status: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM device_commands \
              WHERE device_id = $1 AND command_type = $2 AND status = $3",
        )
        .bind(self.id)
        .bind(command_type)
        .bind(status)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    pub async fn set(&self, pool: &PgPool, assignment_sql: &str) {
        // Hanya dipanggil dari uji dengan potongan SQL literal.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE devices SET {assignment_sql} WHERE id = $1"
        )))
        .bind(self.id)
        .execute(pool)
        .await
        .unwrap();
    }

    pub async fn seen(&self, pool: &PgPool) -> (bool, bool) {
        sqlx::query_as(
            "SELECT last_seen_at IS NOT NULL, first_seen_at IS NOT NULL FROM devices WHERE id = $1",
        )
        .bind(self.id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    pub async fn cleanup(self, pool: &PgPool) {
        // `device_logs.device_id` bersifat RESTRICT: log mentah dihapus lebih dulu.
        sqlx::query("DELETE FROM device_logs WHERE device_id = $1")
            .bind(self.id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM devices WHERE id = $1")
            .bind(self.id)
            .execute(pool)
            .await
            .unwrap();
    }
}

#[derive(Debug)]
pub struct CommandRow {
    pub status: String,
    pub attempts: i16,
    pub return_code: Option<i32>,
    pub result_message: Option<String>,
    pub completed: bool,
}

/// Pegawai uji (dengan departemen sendiri).
pub struct TestEmployee {
    pub id: i64,
    pub department_id: i64,
    /// Nomor pegawai unik (kolom ini unik di seluruh tabel).
    pub employee_number: String,
}

impl TestEmployee {
    /// Pegawai dengan nomor unik berisi angka saja (PIN yang sah).
    pub async fn create(pool: &PgPool, name: &str) -> Self {
        Self::insert(pool, unique_number(), name).await
    }

    /// Pegawai dengan nomor yang memuat spasi (PIN yang tidak boleh dikirim ke mesin).
    pub async fn create_with_invalid_pin(pool: &PgPool, name: &str) -> Self {
        Self::insert(pool, format!("{} 34", unique_number()), name).await
    }

    async fn insert(pool: &PgPool, employee_number: String, name: &str) -> Self {
        let code = unique("TA");
        let department_id: i64 = sqlx::query_scalar(
            "INSERT INTO departments (code, name, created_at, updated_at) \
             VALUES ($1, 'Departemen uji ADMS', now() AT TIME ZONE 'UTC', now() AT TIME ZONE 'UTC') \
             RETURNING id",
        )
        .bind(&code)
        .fetch_one(pool)
        .await
        .unwrap();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO employees (employee_number, name, department_id, created_at, updated_at) \
             VALUES ($1, $2, $3, now() AT TIME ZONE 'UTC', now() AT TIME ZONE 'UTC') RETURNING id",
        )
        .bind(&employee_number)
        .bind(name)
        .bind(department_id)
        .fetch_one(pool)
        .await
        .unwrap();
        Self {
            id,
            department_id,
            employee_number,
        }
    }

    pub async fn cleanup(self, pool: &PgPool) {
        // Perintah yang merujuk pegawai ikut terhapus (ON DELETE CASCADE).
        sqlx::query("DELETE FROM employees WHERE id = $1")
            .bind(self.id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM departments WHERE id = $1")
            .bind(self.department_id)
            .execute(pool)
            .await
            .unwrap();
    }
}

/// Periode uji. `periods` unik per `(year, month)` dan dipakai bersama, jadi tiap uji memakai
/// bulan sendiri (lihat komentar di uji yang memakainya).
pub struct TestPeriod {
    pub year: i16,
    pub month: i16,
}

impl TestPeriod {
    pub async fn create(pool: &PgPool, year: i16, month: i16, status: &str) -> Self {
        sqlx::query(
            "INSERT INTO periods (year, month, status, created_at, updated_at)              VALUES ($1, $2, $3, now() AT TIME ZONE 'UTC', now() AT TIME ZONE 'UTC')              ON CONFLICT (year, month) DO UPDATE SET status = EXCLUDED.status",
        )
        .bind(year)
        .bind(month)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
        Self { year, month }
    }

    pub async fn cleanup(self, pool: &PgPool) {
        sqlx::query("DELETE FROM periods WHERE year = $1 AND month = $2")
            .bind(self.year)
            .bind(self.month)
            .execute(pool)
            .await
            .unwrap();
    }
}

impl TestEmployee {
    pub async fn set_active(&self, pool: &PgPool, active: bool) {
        sqlx::query("UPDATE employees SET is_active = $2 WHERE id = $1")
            .bind(self.id)
            .bind(active)
            .execute(pool)
            .await
            .unwrap();
    }
}

/// Server ADMS sungguhan di port acak (`127.0.0.1:0`) agar simulator berbicara lewat TCP.
/// Dihentikan saat dilepas.
pub struct Server {
    pub addr: std::net::SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl Server {
    /// `host:port` untuk simulator.
    pub fn authority(&self) -> String {
        self.addr.to_string()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub async fn spawn(router: Router) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gagal membuka port uji");
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = http::serve(listener, router, std::future::pending::<()>()).await;
    });
    Server { addr, handle }
}
