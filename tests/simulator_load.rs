//! Uji beban sederhana (Task 6): 100 mesin simulasi berbicara bersamaan lewat TCP sungguhan ke
//! server ADMS dengan kolam database berukuran bawaan produksi (10 koneksi). Hasil dicetak
//! (`cargo test --test simulator_load -- --nocapture`) dan dicatat di `docs/load-test.md`.
//!
//! Batas yang ditegakkan sengaja longgar agar tidak rapuh di mesin CI yang lambat: tanpa galat,
//! semua data masuk persis sekali, dan p99 di bawah 5 detik.

mod common;

use std::time::{Duration, Instant};

use chrono::{Duration as Span, NaiveDate};
use common::{TestEmployee, default_ingest, default_queue, pool, spawn};
use eabsensi_adms::http::{self, Limits};
use eabsensi_adms::simulator::{LatencyStats, Machine, MachineConfig, Record, Resend};
use eabsensi_adms::state::{AppState, Biometrics};
use sqlx::postgres::PgPoolOptions;
use tokio::task::JoinSet;

const MACHINES: usize = 100;
const ROWS_PER_MACHINE: usize = 20;
const CYCLES: usize = 15;
const POLLS: usize = 30;

struct Phase {
    name: &'static str,
    elapsed: Duration,
    stats: LatencyStats,
    failures: usize,
}

impl Phase {
    fn print(&self) {
        let report = self.stats.report();
        println!(
            "[{}] {} mesin: {:.2?} total, {:.0} request/detik, {report}, {} siklus gagal",
            self.name,
            MACHINES,
            self.elapsed,
            report.requests as f64 / self.elapsed.as_secs_f64().max(f64::EPSILON),
            self.failures
        );
    }
}

/// Menjalankan `work` untuk semua mesin bersamaan dan menggabungkan statistiknya.
async fn run_phase<F, Fut>(
    name: &'static str,
    machines: Vec<Machine>,
    work: F,
) -> (Phase, Vec<Machine>)
where
    F: Fn(Machine) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = (Machine, usize)> + Send + 'static,
{
    let started = Instant::now();
    let mut tasks = JoinSet::new();
    for machine in machines {
        let work = work.clone();
        tasks.spawn(async move { work(machine).await });
    }

    let mut stats = LatencyStats::default();
    let mut failures = 0;
    let mut machines = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let (mut machine, failed) = joined.unwrap();
        stats.merge(machine.take_stats());
        failures += failed;
        machines.push(machine);
    }
    (
        Phase {
            name,
            elapsed: started.elapsed(),
            stats,
            failures,
        },
        machines,
    )
}

#[tokio::test]
async fn seratus_mesin_mengirim_log_dan_polling_bersamaan() {
    let pool = pool().await;
    let employee = TestEmployee::create(&pool, "Beban").await;

    // Server memakai kolam seukuran bawaan produksi (`ADMS_DB_MAX_CONNECTIONS=10`).
    let server_pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();
    let router = http::router(
        AppState {
            db: server_pool,
            queue: default_queue(),
            ingest: default_ingest(),
            biometrics: Biometrics::disabled(),
        },
        Limits {
            max_body_bytes: 8 * 1024 * 1024,
            request_timeout: Duration::from_secs(30),
        },
    );
    let server = spawn(router).await;

    let prefix = format!("TEST-ADMS-LOAD-{}", std::process::id());
    sqlx::query(
        "INSERT INTO devices (serial_number, name, created_at, updated_at) \
         SELECT $1 || '-' || lpad(n::text, 3, '0'), 'Mesin beban', now(), now() \
           FROM generate_series(0, $2) AS n",
    )
    .bind(&prefix)
    .bind(i32::try_from(MACHINES).unwrap() - 1)
    .execute(&pool)
    .await
    .unwrap();

    let base = NaiveDate::from_ymd_opt(2022, 6, 1)
        .unwrap()
        .and_hms_opt(7, 0, 0)
        .unwrap();
    let machines: Vec<Machine> = (0..MACHINES)
        .map(|index| {
            let mut config = MachineConfig::new(format!("{prefix}-{index:03}"));
            config.resend = Resend::All;
            let mut machine = Machine::new(server.authority(), config);
            for row in 0..ROWS_PER_MACHINE {
                let offset = i64::try_from(index * ROWS_PER_MACHINE + row).unwrap();
                machine.add_record(Record::new(
                    employee.employee_number.clone(),
                    base + Span::minutes(offset),
                ));
            }
            machine
        })
        .collect();

    // Fase 1: siklus penuh (handshake, unggah ulang seluruh ingatan, polling, hasil perintah).
    let (cycles, machines) = run_phase("siklus penuh", machines, |mut machine| async move {
        let mut failed = 0;
        for _ in 0..CYCLES {
            if machine.cycle().await.is_err() {
                failed += 1;
            }
        }
        (machine, failed)
    })
    .await;
    cycles.print();

    // Fase 2: polling murni (`getrequest`), beban terus-menerus yang sebenarnya dominan.
    let (polling, _machines) = run_phase("polling", machines, |mut machine| async move {
        let mut failed = 0;
        for _ in 0..POLLS {
            if machine.poll().await.is_err() {
                failed += 1;
            }
        }
        (machine, failed)
    })
    .await;
    polling.print();

    // Semua data masuk persis sekali walau tiap mesin mengirim ulang ingatannya belasan kali.
    let expected = i64::try_from(MACHINES * ROWS_PER_MACHINE).unwrap();
    let logs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM device_logs WHERE device_id IN \
         (SELECT id FROM devices WHERE serial_number LIKE $1 || '-%')",
    )
    .bind(&prefix)
    .fetch_one(&pool)
    .await
    .unwrap();
    let punches: i64 =
        sqlx::query_scalar("SELECT count(*) FROM attendance_punches WHERE employee_id = $1")
            .bind(employee.id)
            .fetch_one(&pool)
            .await
            .unwrap();

    sqlx::query(
        "DELETE FROM device_logs WHERE device_id IN \
         (SELECT id FROM devices WHERE serial_number LIKE $1 || '-%')",
    )
    .bind(&prefix)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM devices WHERE serial_number LIKE $1 || '-%'")
        .bind(&prefix)
        .execute(&pool)
        .await
        .unwrap();
    employee.cleanup(&pool).await;

    assert_eq!(
        (logs, punches),
        (expected, expected),
        "log dan punch harus persis sekali"
    );
    for phase in [&cycles, &polling] {
        let report = phase.stats.report();
        assert_eq!(report.errors, 0, "[{}] galat jaringan", phase.name);
        assert_eq!(phase.failures, 0, "[{}] siklus gagal", phase.name);
        assert!(
            report.p99 < Duration::from_secs(5),
            "[{}] p99 terlalu tinggi: {report}",
            phase.name
        );
    }
}
