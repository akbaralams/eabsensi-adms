//! Aturan bersama penentu hasil sebuah log mesin (`docs/schema-proposal.md` bagian 4).
//!
//! Urutan dan hasilnya harus identik dengan `ReprocessDeviceLog::evaluate()` di eabsensi-v2 dan
//! diuji dengan berkas kasus yang sama (`tests/fixtures/outcome-cases.json`, disalin dari
//! `eabsensi-v2/tests/contract/adms/outcome-cases.json`). Fungsi di sini murni: data yang
//! dibutuhkan (pegawai, periode terkunci) disiapkan pemanggil sekali per batch.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, NaiveDateTime, Utc};

use crate::state::IngestRules;

/// Nilai kolom `device_logs.outcome`; sama dengan `DeviceLogOutcome` di eabsensi-v2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Punched,
    UnknownPin,
    EmployeeInactive,
    PeriodClosed,
    OutOfRange,
    Duplicate,
}

impl Outcome {
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Punched => "punched",
            Self::UnknownPin => "unknown_pin",
            Self::EmployeeInactive => "employee_inactive",
            Self::PeriodClosed => "period_closed",
            Self::OutOfRange => "out_of_range",
            Self::Duplicate => "duplicate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Employee {
    pub id: i64,
    pub active: bool,
}

/// Bulan (tahun, bulan) berstatus bukan `open` (`closing` atau `closed`) di tabel `periods`.
pub type LockedMonths = HashSet<(i32, u32)>;

/// Pegawai menurut `employee_number`, dicocokkan persis (tanpa trim dan padding, D9).
pub type Employees = HashMap<String, Employee>;

/// Hasil tahap pertama. Pemeriksaan duplikat butuh data punch sehingga dipisah: pemanggil
/// menjawabnya (dari database atau dari himpunan uji) lalu memanggil [`Verdict::resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Final {
        outcome: Outcome,
        employee_id: Option<i64>,
    },
    /// Semua syarat lolos; tinggal memeriksa apakah sudah ada punch `approved` yang sama.
    CheckDuplicate { employee_id: i64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub outcome: Outcome,
    pub employee_id: Option<i64>,
}

impl Verdict {
    pub fn resolve(self, duplicate_exists: impl FnOnce(i64) -> bool) -> Decision {
        match self {
            Self::Final {
                outcome,
                employee_id,
            } => Decision {
                outcome,
                employee_id,
            },
            Self::CheckDuplicate { employee_id } => Decision {
                outcome: if duplicate_exists(employee_id) {
                    Outcome::Duplicate
                } else {
                    Outcome::Punched
                },
                employee_id: Some(employee_id),
            },
        }
    }
}

/// Urutan (penting):
/// 1. waktu mesin < batas bawah → `out_of_range` (pegawai tidak dicari);
/// 2. tidak ada pegawai dengan nomor yang sama persis → `unknown_pin`;
/// 3. pegawai nonaktif → `employee_inactive`;
/// 4. bulan `punched_at` di zona bisnis berstatus `closing`/`closed` → `period_closed`;
/// 5. sudah ada punch `approved` yang sama → `duplicate` (diputuskan pemanggil);
/// 6. selain itu → `punched`.
pub fn evaluate(
    pin: &str,
    device_time: NaiveDateTime,
    punched_at: DateTime<Utc>,
    rules: &IngestRules,
    employees: &Employees,
    locked: &LockedMonths,
) -> Verdict {
    if device_time < rules.min_device_time {
        return Verdict::Final {
            outcome: Outcome::OutOfRange,
            employee_id: None,
        };
    }

    let Some(employee) = employees.get(pin) else {
        return Verdict::Final {
            outcome: Outcome::UnknownPin,
            employee_id: None,
        };
    };

    if !employee.active {
        return Verdict::Final {
            outcome: Outcome::EmployeeInactive,
            employee_id: Some(employee.id),
        };
    }

    let local = punched_at.with_timezone(&rules.attendance_zone);
    if locked.contains(&(local.year(), local.month())) {
        return Verdict::Final {
            outcome: Outcome::PeriodClosed,
            employee_id: Some(employee.id),
        };
    }

    Verdict::CheckDuplicate {
        employee_id: employee.id,
    }
}
