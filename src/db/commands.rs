use std::time::Duration;

use sqlx::PgPool;

use crate::protocol::command::{CommandKind, batch_limit};

/// Perintah yang baru saja diambil untuk dikirim ke mesin.
#[derive(Debug, Clone)]
pub struct Claimed {
    pub id: i64,
    pub command_type: String,
    pub employee_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct EmployeeRow {
    pub id: i64,
    pub employee_number: String,
    pub name: String,
}

/// Perintah `sent` yang tidak dijawab dalam `ack_timeout` kembali `pending`; setelah
/// `max_attempts` kali dikirim ia ditandai `failed`. Menggantikan kirim-ulang tanpa batas di
/// `commands_user` lama.
pub async fn expire_unanswered(
    pool: &PgPool,
    device_id: i64,
    ack_timeout: Duration,
    max_attempts: i16,
) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        r#"UPDATE device_commands
              SET status = CASE WHEN attempts >= $3 THEN 'failed' ELSE 'pending' END,
                  completed_at = CASE WHEN attempts >= $3 THEN now() END,
                  result_message = CASE WHEN attempts >= $3 THEN 'tidak ada jawaban dari mesin' END,
                  updated_at = now() AT TIME ZONE 'UTC'
            WHERE device_id = $1
              AND status = 'sent'
              AND sent_at < now() - make_interval(secs => $2)"#,
        device_id,
        ack_timeout.as_secs_f64(),
        max_attempts
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// `available_at` bertipe `timestamptz(0)`, jadi nilai bawaan `now()` dibulatkan ke detik terdekat dan
/// bisa tampak sampai 0,5 detik di masa depan; toleransi satu detik di bawah menghindari perintah
/// yang baru dibuat "terlambat" terlihat.
///
/// Mengambil satu kelas prioritas terkecil yang punya perintah menunggu (batas per poll sesuai
/// kode lama) dan menandainya `sent`. `SKIP LOCKED` menjaga dua poll bersamaan tidak
/// mengambil perintah yang sama. Hasil diurutkan menurut id.
pub async fn claim_batch(pool: &PgPool, device_id: i64) -> sqlx::Result<Vec<Claimed>> {
    let priority = sqlx::query_scalar!(
        r#"SELECT priority
             FROM device_commands
            WHERE device_id = $1 AND status = 'pending' AND available_at <= now() + interval '1 second'
            ORDER BY priority
            LIMIT 1"#,
        device_id
    )
    .fetch_optional(pool)
    .await?;

    let Some(priority) = priority else {
        return Ok(Vec::new());
    };

    let mut claimed = sqlx::query_as!(
        Claimed,
        r#"UPDATE device_commands AS d
              SET status = 'sent',
                  sent_at = now(),
                  attempts = d.attempts + 1,
                  updated_at = now() AT TIME ZONE 'UTC'
             FROM (SELECT id
                     FROM device_commands
                    WHERE device_id = $1
                      AND status = 'pending'
                      AND priority = $2
                      AND available_at <= now() + interval '1 second'
                    ORDER BY id
                    LIMIT $3
                      FOR UPDATE SKIP LOCKED) AS picked
            WHERE d.id = picked.id
        RETURNING d.id AS "id!", d.command_type AS "command_type!", d.employee_id"#,
        device_id,
        priority,
        batch_limit(priority)
    )
    .fetch_all(pool)
    .await?;

    claimed.sort_by_key(|command| command.id);
    Ok(claimed)
}

pub async fn employees_by_id(pool: &PgPool, ids: &[i64]) -> sqlx::Result<Vec<EmployeeRow>> {
    sqlx::query_as!(
        EmployeeRow,
        r#"SELECT id, employee_number, name FROM employees WHERE id = ANY($1)"#,
        ids
    )
    .fetch_all(pool)
    .await
}

/// Perintah yang tidak bisa dirakit (pegawai hilang, PIN tidak aman, jenis belum didukung)
/// ditandai gagal agar tidak mengantre selamanya dan terlihat di UI v2.
pub async fn fail(pool: &PgPool, command_id: i64, message: &str) -> sqlx::Result<()> {
    sqlx::query!(
        r#"UPDATE device_commands
              SET status = 'failed',
                  result_message = left($2, 255),
                  completed_at = now(),
                  updated_at = now() AT TIME ZONE 'UTC'
            WHERE id = $1"#,
        command_id,
        message
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Mencatat jawaban mesin. Dibatasi ke mesin pemilik perintah (jawaban mesin lain atas id yang
/// sama diabaikan) dan ke perintah yang belum selesai (jawaban kedua, mis. `set_time`, tidak
/// menimpa). Mengembalikan apakah ada baris yang berubah.
pub async fn complete(
    pool: &PgPool,
    device_id: i64,
    command_id: i64,
    return_code: Option<i32>,
) -> sqlx::Result<bool> {
    let success = return_code == Some(0);
    let status = if success { "succeeded" } else { "failed" };
    let message = match (success, return_code) {
        (true, _) => None,
        (false, Some(code)) => Some(format!("mesin melaporkan Return={code}")),
        (false, None) => Some("mesin melaporkan Return yang tidak terbaca".to_owned()),
    };

    let result = sqlx::query!(
        r#"UPDATE device_commands
              SET status = $3,
                  return_code = $4,
                  result_message = $5,
                  completed_at = now(),
                  updated_at = now() AT TIME ZONE 'UTC'
            WHERE id = $1
              AND device_id = $2
              AND status IN ('sent', 'pending')"#,
        command_id,
        device_id,
        status,
        return_code,
        message
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Mengantrekan perintah pemeliharaan (`info`, `check`, `set_time`). Index unik parsial
/// menjamin paling banyak satu yang aktif per mesin.
pub async fn enqueue_maintenance(
    pool: &PgPool,
    device_id: i64,
    kind: CommandKind,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"INSERT INTO device_commands
                   (device_id, command_type, priority, status, created_at, updated_at)
            VALUES ($1, $2, $3, 'pending', now() AT TIME ZONE 'UTC', now() AT TIME ZONE 'UTC')
       ON CONFLICT (device_id, command_type)
                WHERE status IN ('pending', 'sent') AND command_type IN ('info', 'check', 'set_time')
         DO NOTHING"#,
        device_id,
        kind.as_db(),
        kind.priority()
    )
    .execute(pool)
    .await?;
    Ok(())
}
