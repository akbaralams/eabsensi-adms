//! Log mentah mesin (`device_logs`), pegawai, periode, dan punch yang dibutuhkan penerimaan ATTLOG.
//! Semua fungsi menerima koneksi dari transaksi batch.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::PgConnection;

use crate::ingest::outcome::{Employee, Employees, LockedMonths};

/// Kunci idempotensi yang sudah ada: `(pin, waktu lokal mesin)` untuk mesin ini.
pub async fn existing_keys(
    conn: &mut PgConnection,
    device_id: i64,
    pins: &[String],
    times: &[NaiveDateTime],
) -> sqlx::Result<HashSet<(String, NaiveDateTime)>> {
    let rows = sqlx::query!(
        r#"SELECT l.pin AS "pin!", l.device_time AS "device_time!"
             FROM device_logs l
             JOIN UNNEST($2::text[], $3::timestamp[]) AS t(pin, device_time)
               ON l.pin = t.pin AND l.device_time = t.device_time
            WHERE l.device_id = $1"#,
        device_id,
        pins,
        times
    )
    .fetch_all(conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| (row.pin, row.device_time))
        .collect())
}

/// Pegawai menurut `employee_number`, cocok persis.
pub async fn employees_by_numbers(
    conn: &mut PgConnection,
    numbers: &[String],
) -> sqlx::Result<Employees> {
    let rows = sqlx::query!(
        r#"SELECT id, employee_number, is_active
             FROM employees
            WHERE employee_number = ANY($1)"#,
        numbers
    )
    .fetch_all(conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.employee_number,
                Employee {
                    id: row.id,
                    active: row.is_active,
                },
            )
        })
        .collect::<HashMap<_, _>>())
}

/// Bulan yang terkunci untuk punch: periode berstatus `closing` atau `closed`
/// (sama dengan `lockedMonthsBetween` di eabsensi-v2).
pub async fn locked_months(conn: &mut PgConnection) -> sqlx::Result<LockedMonths> {
    let rows = sqlx::query!(r#"SELECT year, month FROM periods WHERE status <> 'open'"#)
        .fetch_all(conn)
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(|row| Some((i32::from(row.year), u32::try_from(row.month).ok()?)))
        .collect())
}

pub async fn approved_punch_exists(
    conn: &mut PgConnection,
    employee_id: i64,
    punched_at: DateTime<Utc>,
) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM attendance_punches
                WHERE employee_id = $1 AND punched_at = $2 AND status = 'approved'
           ) AS "exists!""#,
        employee_id,
        punched_at
    )
    .fetch_one(conn)
    .await
}

pub struct NewLog<'a> {
    pub device_id: i64,
    pub pin: &'a str,
    pub device_time: NaiveDateTime,
    pub punched_at: DateTime<Utc>,
    pub io_status: Option<i16>,
    pub verify_mode: Option<i16>,
    pub received_at: DateTime<Utc>,
    pub source_ip: Option<&'a str>,
    pub employee_id: Option<i64>,
    pub outcome: &'static str,
    pub is_suspect: bool,
}

/// Menyimpan log mentah. `None` berarti kunci `(mesin, pin, waktu)` sudah ada (kiriman ulang,
/// termasuk yang bersamaan dari koneksi lain): tidak ada yang ditambahkan.
pub async fn insert_log(conn: &mut PgConnection, log: &NewLog<'_>) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        r#"INSERT INTO device_logs
                  (device_id, pin, device_time, punched_at, io_status, verify_mode, received_at,
                   source_ip, employee_id, outcome, is_suspect, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                   now() AT TIME ZONE 'UTC', now() AT TIME ZONE 'UTC')
      ON CONFLICT (device_id, pin, device_time) DO NOTHING
        RETURNING id"#,
        log.device_id,
        log.pin,
        log.device_time,
        log.punched_at,
        log.io_status,
        log.verify_mode,
        log.received_at,
        log.source_ip,
        log.employee_id,
        log.outcome,
        log.is_suspect
    )
    .fetch_optional(conn)
    .await
}

/// Punch dari mesin: `source = device`, langsung `approved` (D3), tanpa foto/lokasi.
pub async fn insert_punch(
    conn: &mut PgConnection,
    employee_id: i64,
    punched_at: DateTime<Utc>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"INSERT INTO attendance_punches
                  (employee_id, punched_at, source, status, created_at, updated_at)
           VALUES ($1, $2, 'device', 'approved',
                   now() AT TIME ZONE 'UTC', now() AT TIME ZONE 'UTC')
        RETURNING id"#,
        employee_id,
        punched_at
    )
    .fetch_one(conn)
    .await
}

pub async fn link_punch(conn: &mut PgConnection, log_id: i64, punch_id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        r#"UPDATE device_logs
              SET punch_id = $2, updated_at = now() AT TIME ZONE 'UTC'
            WHERE id = $1"#,
        log_id,
        punch_id
    )
    .execute(conn)
    .await?;
    Ok(())
}
