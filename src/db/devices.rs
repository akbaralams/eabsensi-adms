use sqlx::PgPool;

use crate::protocol::info::DeviceInfo;

/// Baris `devices` yang dibutuhkan untuk menjawab mesin.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: i64,
    pub serial_number: String,
    pub device_type: String,
    pub push_version: Option<String>,
    pub initialized: bool,
    pub timezone: String,
    pub poll_delay_seconds: i16,
    pub error_delay_seconds: i16,
    pub attlog_stamp: Option<String>,
    pub operlog_stamp: Option<String>,
    pub photo_stamp: Option<String>,
    pub biodata_stamp: Option<String>,
}

/// Mesin terdaftar dan aktif dengan nomor seri itu. Mesin tak dikenal atau nonaktif tidak
/// dikembalikan (keputusan D2: ditolak).
pub async fn find_active(pool: &PgPool, serial: &str) -> sqlx::Result<Option<Device>> {
    sqlx::query_as!(
        Device,
        r#"SELECT id,
                  serial_number,
                  device_type,
                  push_version,
                  initialized_at IS NOT NULL AS "initialized!",
                  timezone,
                  poll_delay_seconds,
                  error_delay_seconds,
                  attlog_stamp,
                  operlog_stamp,
                  photo_stamp,
                  biodata_stamp
             FROM devices
            WHERE serial_number = $1 AND is_active"#,
        serial
    )
    .fetch_optional(pool)
    .await
}

/// Heartbeat: setiap request mesin yang sah memperbarui `last_seen_at`.
pub async fn touch(pool: &PgPool, device_id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        r#"UPDATE devices
              SET last_seen_at = now(),
                  first_seen_at = COALESCE(first_seen_at, now()),
                  updated_at = now() AT TIME ZONE 'UTC'
            WHERE id = $1"#,
        device_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_push_version(pool: &PgPool, device_id: i64, version: &str) -> sqlx::Result<()> {
    sqlx::query!(
        r#"UPDATE devices
              SET push_version = $2, updated_at = now() AT TIME ZONE 'UTC'
            WHERE id = $1 AND push_version IS DISTINCT FROM $2"#,
        device_id,
        version
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_att(pool: &PgPool, device_id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        r#"UPDATE devices
              SET device_type = 'att', updated_at = now() AT TIME ZONE 'UTC'
            WHERE id = $1 AND device_type <> 'att'"#,
        device_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_initialized(pool: &PgPool, device_id: i64) -> sqlx::Result<()> {
    sqlx::query!(
        r#"UPDATE devices
              SET initialized_at = now(), updated_at = now() AT TIME ZONE 'UTC'
            WHERE id = $1 AND initialized_at IS NULL"#,
        device_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Menyimpan laporan mesin. Nilai yang tidak dilaporkan tidak menimpa nilai lama.
pub async fn update_info(
    pool: &PgPool,
    device_id: i64,
    info: &DeviceInfo,
    reported_options: Option<&serde_json::Value>,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"UPDATE devices
              SET firmware_version  = COALESCE($2, firmware_version),
                  push_version      = COALESCE($3, push_version),
                  ip_address        = COALESCE($4, ip_address),
                  model             = COALESCE($5, model),
                  platform          = COALESCE($6, platform),
                  user_count        = COALESCE($7, user_count),
                  fingerprint_count = COALESCE($8, fingerprint_count),
                  face_count        = COALESCE($9, face_count),
                  log_count         = COALESCE($10, log_count),
                  reported_options  = COALESCE($11::jsonb, reported_options),
                  updated_at        = now() AT TIME ZONE 'UTC'
            WHERE id = $1"#,
        device_id,
        info.firmware_version.as_deref(),
        info.push_version.as_deref(),
        info.ip_address.as_deref(),
        info.model.as_deref(),
        info.platform.as_deref(),
        info.user_count,
        info.fingerprint_count,
        info.face_count,
        info.log_count,
        reported_options
    )
    .execute(pool)
    .await?;
    Ok(())
}
