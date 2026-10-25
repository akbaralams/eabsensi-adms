//! Rotasi kunci biometrik (`eabsensi-adms rekey-biometrics`): membuka tiap template dengan
//! kunci lamanya dan mengenkripsinya ulang dengan kunci terkini.
//!
//! Tanpa ini, baris lama baru berpindah kunci ketika mesin kebetulan mengirim ulang templatenya;
//! kunci lama baru boleh dibuang dari `BIOMETRIC_OLD_KEYS` setelah perintah ini melaporkan nol
//! baris tersisa. Isi template hanya ada di memori dan tidak pernah dicetak.

use sqlx::PgPool;

use crate::db::biometrics;
use crate::vault::{Aad, Vault};

const BATCH: i64 = 200;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Baris yang dienkripsi ulang ke kunci terkini.
    pub resealed: usize,
    /// Dengan `--dry-run`: baris yang akan dienkripsi ulang (semuanya sudah terbukti bisa dibuka).
    pub would_reseal: usize,
    /// Baris yang tidak bisa dibuka (kunci lamanya hilang dari konfigurasi atau data rusak);
    /// tidak diubah.
    pub failed: Vec<i64>,
    /// Baris yang ditimpa mesin selama proses berjalan; dilewati karena sudah baru.
    pub overwritten: usize,
}

pub async fn run(
    pool: &PgPool,
    vault: &Vault,
    from: Option<&str>,
    dry_run: bool,
) -> sqlx::Result<Report> {
    let mut report = Report::default();
    let mut after_id = 0;

    loop {
        let batch =
            biometrics::not_under_key(pool, vault.current_key_id(), from, after_id, BATCH).await?;
        let Some(last) = batch.last() else {
            return Ok(report);
        };
        after_id = last.id;

        let mut tx = pool.begin().await?;
        for stored in &batch {
            let aad = Aad {
                employee_id: stored.employee_id,
                kind: &stored.kind,
                finger_index: stored.finger_index,
                algorithm_version: &stored.algorithm_version,
            };
            let Ok(plain) = vault.decrypt(&aad, &stored.ciphertext, &stored.nonce, &stored.key_id)
            else {
                report.failed.push(stored.id);
                continue;
            };

            if dry_run {
                report.would_reseal += 1;
                continue;
            }

            let Ok(sealed) = vault.encrypt(&aad, &plain) else {
                report.failed.push(stored.id);
                continue;
            };
            let updated = biometrics::reseal(
                &mut tx,
                stored.id,
                &stored.key_id,
                &sealed.ciphertext,
                &sealed.nonce,
                &sealed.key_id,
            )
            .await?;
            if updated {
                report.resealed += 1;
            } else {
                report.overwritten += 1;
            }
        }
        tx.commit().await?;
    }
}

pub fn render(report: &Report, dry_run: bool, current_key_id: &str) -> String {
    let mut out = String::new();
    if dry_run {
        out.push_str(&format!(
            "DRY-RUN: {} baris akan dienkripsi ulang ke kunci {current_key_id}\n",
            report.would_reseal
        ));
    } else {
        out.push_str(&format!(
            "{} baris dienkripsi ulang ke kunci {current_key_id}\n",
            report.resealed
        ));
        if report.overwritten > 0 {
            out.push_str(&format!(
                "{} baris ditimpa mesin selama proses berjalan (sudah memakai kunci terkini)\n",
                report.overwritten
            ));
        }
    }
    if report.failed.is_empty() {
        out.push_str("semua baris yang diperiksa bisa dibuka\n");
    } else {
        // Hanya id baris; tidak ada isi atau rincian kunci.
        let ids: Vec<String> = report.failed.iter().map(i64::to_string).collect();
        out.push_str(&format!(
            "GAGAL: {} baris tidak bisa dibuka (id: {}); periksa BIOMETRIC_OLD_KEYS memuat kunci lamanya\n",
            report.failed.len(),
            ids.join(", ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laporan_untuk_operator() {
        let ok = Report {
            resealed: 12,
            ..Report::default()
        };
        assert_eq!(
            render(&ok, false, "k2"),
            "12 baris dienkripsi ulang ke kunci k2\nsemua baris yang diperiksa bisa dibuka\n"
        );

        let dry = Report {
            would_reseal: 3,
            ..Report::default()
        };
        assert!(render(&dry, true, "k2").starts_with("DRY-RUN: 3 baris"));

        let bad = Report {
            resealed: 1,
            overwritten: 2,
            failed: vec![7, 9],
            ..Report::default()
        };
        let text = render(&bad, false, "k2");
        assert!(text.contains("2 baris ditimpa mesin"));
        assert!(text.contains("GAGAL: 2 baris tidak bisa dibuka (id: 7, 9)"));
    }
}
