//! Penerimaan template biometrik terenkripsi (keputusan D1).
//!
//! Hanya pegawai yang dikenal (cocok persis `employee_number`) yang boleh memiliki template;
//! sisanya dihitung dan dibuang tanpa isi. Template yang tidak berubah tidak ditulis ulang
//! (mesin mengirim ulang batch yang sama pada tiap koneksi), kecuali dienkripsi ulang ke kunci
//! terkini setelah rotasi.

use std::collections::HashMap;

use sqlx::PgPool;

use crate::db::biometrics::{self, NewTemplate};
use crate::db::devices::{self, Device, Stamp};
use crate::db::logs;
use crate::protocol::biometric::{Kind, Row};
use crate::vault::{Aad, Vault, VaultError};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Template baru atau berubah yang ditulis (terenkripsi).
    pub stored: usize,
    /// Sudah tersimpan persis sama dan sudah memakai kunci terkini.
    pub unchanged: usize,
    /// PIN tidak cocok dengan pegawai mana pun: dibuang.
    pub unknown_pin: usize,
    /// Foto wajah yang dibuang karena `BIOMETRIC_STORE_FACE_PHOTO` mati.
    pub photos_ignored: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Vault(#[from] VaultError),
}

/// Versi algoritma yang dilaporkan mesin untuk jenis ini; kosong bila tidak diketahui.
fn algorithm_version(device: &Device, kind: Kind) -> String {
    match kind {
        Kind::Fingerprint => device.fp_version.clone(),
        Kind::Face => device.face_version.clone(),
        Kind::FacePhoto => None,
    }
    .unwrap_or_default()
}

/// Menyimpan template dari satu unggahan dalam satu transaksi. `stamp` (bila ada) disimpan di
/// transaksi yang sama.
pub async fn ingest_templates(
    pool: &PgPool,
    device: &Device,
    vault: &Vault,
    store_face_photo: bool,
    rows: Vec<Row>,
    stamp: Option<(Stamp, &str)>,
) -> Result<Summary, Error> {
    let mut summary = Summary::default();
    let mut rows = rows;
    if !store_face_photo {
        let before = rows.len();
        rows.retain(|row| row.kind != Kind::FacePhoto);
        summary.photos_ignored = before - rows.len();
    }

    let mut tx = pool.begin().await?;

    if !rows.is_empty() {
        let mut numbers: Vec<String> = rows.iter().map(|row| row.pin.clone()).collect();
        numbers.sort();
        numbers.dedup();
        let employees = logs::employees_by_numbers(&mut tx, &numbers).await?;

        let employee_ids: Vec<i64> = employees.values().map(|employee| employee.id).collect();
        let mut existing: HashMap<(i64, String, i16, String), biometrics::Stored> = HashMap::new();
        if !employee_ids.is_empty() {
            for stored in biometrics::for_employees(&mut tx, &employee_ids).await? {
                existing.insert(
                    (
                        stored.employee_id,
                        stored.kind.clone(),
                        stored.finger_index,
                        stored.algorithm_version.clone(),
                    ),
                    stored,
                );
            }
        }

        for row in rows {
            let Some(employee) = employees.get(&row.pin) else {
                summary.unknown_pin += 1;
                continue;
            };

            let version = algorithm_version(device, row.kind);
            let aad = Aad {
                employee_id: employee.id,
                kind: row.kind.as_db(),
                finger_index: row.finger_index,
                algorithm_version: &version,
            };
            let key = (
                employee.id,
                row.kind.as_db().to_owned(),
                row.finger_index,
                version.clone(),
            );

            // Sama persis dan sudah memakai kunci terkini: tidak perlu menulis ulang.
            let unchanged = existing.get(&key).is_some_and(|stored| {
                stored.is_valid == row.valid
                    && stored.original_size == row.size
                    && stored.key_id == vault.current_key_id()
                    && vault
                        .decrypt(&aad, &stored.ciphertext, &stored.nonce, &stored.key_id)
                        .is_ok_and(|plain| plain == row.data)
            });
            if unchanged {
                summary.unchanged += 1;
                continue;
            }

            let sealed = vault.encrypt(&aad, &row.data)?;
            biometrics::upsert(
                &mut tx,
                &NewTemplate {
                    employee_id: employee.id,
                    kind: row.kind.as_db(),
                    finger_index: row.finger_index,
                    algorithm_version: &version,
                    is_valid: row.valid,
                    original_size: row.size,
                    ciphertext: &sealed.ciphertext,
                    nonce: &sealed.nonce,
                    key_id: &sealed.key_id,
                    source_device_id: device.id,
                },
            )
            .await?;
            summary.stored += 1;
        }
    }

    if let Some((kind, value)) = stamp {
        devices::set_stamp(&mut *tx, device.id, kind, value).await?;
    }

    tx.commit().await?;
    Ok(summary)
}
