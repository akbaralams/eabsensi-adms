//! Kasus uji bersama dengan eabsensi-v2 (`tests/contract/adms/outcome-cases.json`, disalin
//! persis ke `tests/fixtures/`). Aturan penentu hasil log mesin ditulis dua kali (Laravel dan
//! Rust); berkas yang sama menjaga keduanya tetap identik. Ubah berkas ini hanya lewat PR
//! pasangan di kedua repo.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;
use eabsensi_adms::ingest::outcome::{Employee, Employees, LockedMonths, Outcome, evaluate};
use eabsensi_adms::protocol::attlog::{TIME_FORMAT, to_utc};
use eabsensi_adms::state::IngestRules;
use serde_json::Value;

const CASES: &str = include_str!("fixtures/outcome-cases.json");

fn naive(value: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(value, TIME_FORMAT).unwrap()
}

fn utc(value: &str) -> DateTime<Utc> {
    naive(value).and_utc()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("kunci {key} hilang"))
}

#[test]
fn semua_kasus_bersama_menghasilkan_outcome_yang_sama_dengan_laravel() {
    let doc: Value = serde_json::from_str(CASES).unwrap();

    let rules = IngestRules {
        attendance_zone: text(&doc["config"], "timezone").parse::<Tz>().unwrap(),
        min_device_time: naive(text(&doc["config"], "min_device_time")),
        suspect_after: Duration::from_secs(500),
    };

    let mut employees: Employees = HashMap::new();
    let mut ids: HashMap<String, i64> = HashMap::new();
    for (index, employee) in doc["fixtures"]["employees"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let number = text(employee, "employee_number").to_owned();
        let id = i64::try_from(index).unwrap() + 1;
        ids.insert(number.clone(), id);
        employees.insert(
            number,
            Employee {
                id,
                active: employee["is_active"].as_bool().unwrap(),
            },
        );
    }

    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 18, "berkas kasus bersama menyusut");

    for case in cases {
        let name = text(case, "name");
        let given = &case["given"];

        let zone: Tz = text(given, "device_timezone").parse().unwrap();
        let device_time = naive(text(given, "device_time"));
        let punched_at = to_utc(device_time, zone).expect("waktu uji harus valid");

        // Periode berstatus bukan `open` terkunci (closing dan closed).
        let locked: LockedMonths = given["periods"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|period| text(period, "status") != "open")
            .map(|period| {
                let month = text(period, "month");
                let (year, month) = month.split_once('-').unwrap();
                (year.parse().unwrap(), month.parse().unwrap())
            })
            .collect();

        // Hanya punch `approved` yang membuat log berikutnya menjadi duplikat.
        let approved: HashSet<(i64, DateTime<Utc>)> = given["existing_punches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|punch| text(punch, "status") == "approved")
            .map(|punch| {
                (
                    ids[text(punch, "employee_number")],
                    utc(text(punch, "punched_at_utc")),
                )
            })
            .collect();

        let decision = evaluate(
            text(given, "pin"),
            device_time,
            punched_at,
            &rules,
            &employees,
            &locked,
        )
        .resolve(|employee_id| approved.contains(&(employee_id, punched_at)));

        let expect = &case["expect"];
        let expected_outcome = text(expect, "outcome");
        assert_eq!(decision.outcome.as_db(), expected_outcome, "kasus: {name}");
        assert_eq!(
            decision.outcome == Outcome::Punched,
            expect["punch_created"].as_bool().unwrap(),
            "punch_created, kasus: {name}"
        );
        assert_eq!(
            decision.employee_id.is_some(),
            expect["employee_linked"].as_bool().unwrap(),
            "employee_linked, kasus: {name}"
        );
        if decision.outcome == Outcome::Punched {
            assert_eq!(
                punched_at,
                utc(text(expect, "punched_at_utc")),
                "punched_at_utc, kasus: {name}"
            );
        }
    }
}
