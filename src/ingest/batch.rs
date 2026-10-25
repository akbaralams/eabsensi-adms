//! Penerimaan satu batch ATTLOG dalam satu transaksi.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use sqlx::PgPool;

use super::outcome::{Outcome, Verdict, evaluate};
use crate::db::devices::{self, Stamp};
use crate::db::logs::{self, NewLog};
use crate::protocol::attlog::{AttRow, parse_attlog, to_utc};
use crate::state::IngestRules;

/// Ringkasan satu batch, untuk balasan dan log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Baris tak kosong di badan (dipakai untuk balasan `OK:<n>`).
    pub lines: usize,
    /// Baris yang tidak bisa dibaca dan dilewati.
    pub malformed: usize,
    /// Baris yang sudah pernah diterima (kiriman ulang): tidak menambah apa pun.
    pub resent: usize,
    /// Baris baru menurut `outcome`.
    pub outcomes: BTreeMap<&'static str, usize>,
    /// Baris baru yang ditandai `is_suspect`.
    pub suspect: usize,
}

impl Summary {
    pub fn punched(&self) -> usize {
        self.outcomes
            .get(Outcome::Punched.as_db())
            .copied()
            .unwrap_or(0)
    }
}

/// Menyimpan log mentah secara idempoten dan membuat punch untuk yang memenuhi syarat.
///
/// Seluruh batch dalam satu transaksi: kegagalan database membatalkan semuanya dan memanggil
/// error ke pemanggil (mesin tidak diberi `OK` sehingga mengirim ulang). `stamp` (dari parameter
/// `Stamp` mesin) disimpan di transaksi yang sama, jadi stamp tidak pernah maju mendahului data.
pub async fn ingest_attlog(
    pool: &PgPool,
    device_id: i64,
    device_zone: Tz,
    body: &str,
    stamp: Option<&str>,
    rules: &IngestRules,
    source_ip: Option<&str>,
) -> sqlx::Result<Summary> {
    let parsed = parse_attlog(body);
    let mut summary = Summary {
        lines: parsed.lines,
        malformed: parsed.malformed.len(),
        ..Summary::default()
    };

    // Baris yang waktunya jatuh di celah zona waktu tidak bisa diubah ke UTC; dilewati.
    let mut rows: Vec<(AttRow, DateTime<Utc>)> = Vec::with_capacity(parsed.rows.len());
    for row in parsed.rows {
        match to_utc(row.device_time, device_zone) {
            Some(punched_at) => rows.push((row, punched_at)),
            None => summary.malformed += 1,
        }
    }

    let received_at = Utc::now();
    let suspect_limit = received_at
        + TimeDelta::from_std(rules.suspect_after).unwrap_or_else(|_| TimeDelta::seconds(500));

    let mut tx = pool.begin().await?;

    // Kiriman ulang (mesin mengirim ulang batch yang sama pada tiap koneksi) disaring dulu dengan
    // satu query, supaya batch besar yang sudah diterima tidak memicu query per baris.
    let pins: Vec<String> = rows.iter().map(|(row, _)| row.pin.clone()).collect();
    let times: Vec<NaiveDateTime> = rows.iter().map(|(row, _)| row.device_time).collect();
    let existing = if rows.is_empty() {
        HashSet::new()
    } else {
        logs::existing_keys(&mut tx, device_id, &pins, &times).await?
    };
    let total = rows.len();
    rows.retain(|(row, _)| !existing.contains(&(row.pin.clone(), row.device_time)));
    summary.resent = total - rows.len();

    if !rows.is_empty() {
        let mut numbers: Vec<String> = rows.iter().map(|(row, _)| row.pin.clone()).collect();
        numbers.sort();
        numbers.dedup();
        let employees = logs::employees_by_numbers(&mut tx, &numbers).await?;
        let locked = logs::locked_months(&mut tx).await?;

        for (row, punched_at) in rows {
            let verdict = evaluate(
                &row.pin,
                row.device_time,
                punched_at,
                rules,
                &employees,
                &locked,
            );
            let decision = match verdict {
                Verdict::CheckDuplicate { employee_id } => {
                    let exists =
                        logs::approved_punch_exists(&mut tx, employee_id, punched_at).await?;
                    verdict.resolve(|_| exists)
                }
                Verdict::Final { .. } => verdict.resolve(|_| false),
            };

            let is_suspect = punched_at > suspect_limit;
            let log_id = logs::insert_log(
                &mut tx,
                &NewLog {
                    device_id,
                    pin: &row.pin,
                    device_time: row.device_time,
                    punched_at,
                    io_status: row.io_status,
                    verify_mode: row.verify_mode,
                    received_at,
                    source_ip,
                    employee_id: decision.employee_id,
                    outcome: decision.outcome.as_db(),
                    is_suspect,
                },
            )
            .await?;

            // Kunci sama sudah dimasukkan koneksi lain (atau baris kembar di batch ini).
            let Some(log_id) = log_id else {
                summary.resent += 1;
                continue;
            };

            if decision.outcome == Outcome::Punched {
                let Some(employee_id) = decision.employee_id else {
                    continue;
                };
                let punch_id = logs::insert_punch(&mut tx, employee_id, punched_at).await?;
                logs::link_punch(&mut tx, log_id, punch_id).await?;
            }

            *summary
                .outcomes
                .entry(decision.outcome.as_db())
                .or_insert(0) += 1;
            if is_suspect {
                summary.suspect += 1;
            }
        }
    }

    if let Some(value) = stamp {
        devices::set_stamp(&mut *tx, device_id, Stamp::AttLog, value).await?;
    }

    tx.commit().await?;
    Ok(summary)
}
