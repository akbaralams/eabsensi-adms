//! `biometric_templates`: hanya ADMS yang menulis dan membaca isi (ciphertext). v2 hanya melihat
//! metadata.

use sqlx::{PgConnection, PgPool};

/// Satu baris tersimpan, termasuk ciphertext. Jangan dicetak; `Debug` menyembunyikan isinya.
#[derive(Clone)]
pub struct Stored {
    pub id: i64,
    pub employee_id: i64,
    pub kind: String,
    pub finger_index: i16,
    pub algorithm_version: String,
    pub is_valid: bool,
    pub original_size: i32,
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub key_id: String,
}

impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stored")
            .field("id", &self.id)
            .field("employee_id", &self.employee_id)
            .field("kind", &self.kind)
            .field("finger_index", &self.finger_index)
            .finish_non_exhaustive()
    }
}

/// Template yang sudah ada untuk sekumpulan pegawai (untuk melewati yang tidak berubah).
pub async fn for_employees(
    conn: &mut PgConnection,
    employee_ids: &[i64],
) -> sqlx::Result<Vec<Stored>> {
    sqlx::query_as!(
        Stored,
        r#"SELECT id, employee_id, kind, finger_index, algorithm_version, is_valid,
                  original_size, ciphertext, nonce, key_id
             FROM biometric_templates
            WHERE employee_id = ANY($1)"#,
        employee_ids
    )
    .fetch_all(conn)
    .await
}

pub async fn find(pool: &PgPool, id: i64) -> sqlx::Result<Option<Stored>> {
    sqlx::query_as!(
        Stored,
        r#"SELECT id, employee_id, kind, finger_index, algorithm_version, is_valid,
                  original_size, ciphertext, nonce, key_id
             FROM biometric_templates
            WHERE id = $1"#,
        id
    )
    .fetch_optional(pool)
    .await
}

pub struct NewTemplate<'a> {
    pub employee_id: i64,
    pub kind: &'a str,
    pub finger_index: i16,
    pub algorithm_version: &'a str,
    pub is_valid: bool,
    pub original_size: i32,
    pub ciphertext: &'a [u8],
    pub nonce: &'a [u8],
    pub key_id: &'a str,
    pub source_device_id: i64,
}

/// Menyimpan atau menimpa template untuk `(pegawai, jenis, jari, versi algoritma)`.
pub async fn upsert(conn: &mut PgConnection, template: &NewTemplate<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        r#"INSERT INTO biometric_templates
                  (employee_id, kind, finger_index, algorithm_version, is_valid, original_size,
                   ciphertext, nonce, key_id, source_device_id, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                   now() AT TIME ZONE 'UTC', now() AT TIME ZONE 'UTC')
      ON CONFLICT (employee_id, kind, finger_index, algorithm_version)
        DO UPDATE SET is_valid         = EXCLUDED.is_valid,
                      original_size    = EXCLUDED.original_size,
                      ciphertext       = EXCLUDED.ciphertext,
                      nonce            = EXCLUDED.nonce,
                      key_id           = EXCLUDED.key_id,
                      source_device_id = EXCLUDED.source_device_id,
                      updated_at       = now() AT TIME ZONE 'UTC'"#,
        template.employee_id,
        template.kind,
        template.finger_index,
        template.algorithm_version,
        template.is_valid,
        template.original_size,
        template.ciphertext,
        template.nonce,
        template.key_id,
        template.source_device_id
    )
    .execute(conn)
    .await?;
    Ok(())
}
