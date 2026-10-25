//! Simulator mesin ZKTeco untuk pengembangan dan uji beban.
//!
//! Contoh (mesin harus sudah terdaftar di tabel `devices`, lihat README):
//!
//! ```text
//! adms-sim --url http://127.0.0.1:8090 --serial SN-0001 --rows 20 --cycles 3
//! adms-sim --url http://127.0.0.1:8090 --serial LOAD --machines 100 --cycles 20 --delay-ms 0
//! ```

use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use chrono::NaiveDate;
use eabsensi_adms::simulator::{LatencyStats, Machine, MachineConfig, Record, Resend};
use tokio::task::JoinSet;

const USAGE: &str = "pemakaian: adms-sim --url http://host:port --serial SN \
[--machines N] [--cycles N] [--rows N] [--pin PIN] [--delay-ms MS] [--resend all|stamp] [--device-type att]";

struct Args {
    authority: String,
    serial: String,
    machines: usize,
    cycles: usize,
    rows: usize,
    pin: String,
    delay: Duration,
    resend: Resend,
    device_type: Option<String>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        authority: String::new(),
        serial: String::new(),
        machines: 1,
        cycles: 1,
        rows: 0,
        pin: "1".to_owned(),
        delay: Duration::ZERO,
        resend: Resend::All,
        device_type: None,
    };

    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        if flag == "--help" || flag == "-h" {
            println!("{USAGE}");
            std::process::exit(0);
        }
        let value = iter
            .next()
            .with_context(|| format!("{flag} butuh nilai\n{USAGE}"))?;
        match flag.as_str() {
            "--url" => {
                args.authority = value
                    .trim_start_matches("http://")
                    .trim_end_matches('/')
                    .to_owned();
            }
            "--serial" => args.serial = value,
            "--machines" => args.machines = value.parse().context("--machines bukan angka")?,
            "--cycles" => args.cycles = value.parse().context("--cycles bukan angka")?,
            "--rows" => args.rows = value.parse().context("--rows bukan angka")?,
            "--pin" => args.pin = value,
            "--delay-ms" => {
                args.delay =
                    Duration::from_millis(value.parse().context("--delay-ms bukan angka")?);
            }
            "--resend" => {
                args.resend = match value.as_str() {
                    "all" => Resend::All,
                    "stamp" => Resend::FromStamp,
                    _ => bail!("--resend harus all atau stamp"),
                }
            }
            "--device-type" => args.device_type = Some(value),
            other => bail!("opsi tidak dikenal: {other}\n{USAGE}"),
        }
    }

    if args.authority.is_empty() || args.serial.is_empty() {
        bail!("{USAGE}");
    }
    if args.machines == 0 || args.cycles == 0 {
        bail!("--machines dan --cycles harus lebih dari 0");
    }
    Ok(args)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let started = Instant::now();
    let mut tasks = JoinSet::new();

    for index in 0..args.machines {
        let serial = if args.machines == 1 {
            args.serial.clone()
        } else {
            format!("{}-{index:04}", args.serial)
        };
        let mut config = MachineConfig::new(serial);
        config.resend = args.resend;
        config.device_type = args.device_type.clone();

        let mut machine = Machine::new(args.authority.clone(), config);
        // Waktu log unik per mesin supaya tidak saling menjadi duplikat.
        let base = NaiveDate::from_ymd_opt(2024, 1, 1)
            .and_then(|date| date.and_hms_opt(7, 0, 0))
            .context("tanggal dasar tidak valid")?;
        for row in 0..args.rows {
            let offset = i64::try_from(index * args.rows + row).unwrap_or(0);
            machine.add_record(Record::new(
                args.pin.clone(),
                base + chrono::Duration::minutes(offset),
            ));
        }

        let (cycles, delay) = (args.cycles, args.delay);
        tasks.spawn(async move {
            let mut rejected = false;
            let mut failures = 0usize;
            for _ in 0..cycles {
                match machine.cycle().await {
                    Ok(report) => rejected |= report.rejected,
                    Err(error) => {
                        failures += 1;
                        eprintln!("{}: {error}", machine.serial());
                    }
                }
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
            }
            (machine.take_stats(), rejected, failures)
        });
    }

    let mut stats = LatencyStats::default();
    let (mut rejected_machines, mut failed_cycles) = (0usize, 0usize);
    while let Some(joined) = tasks.join_next().await {
        let (machine_stats, rejected, failures) =
            joined.context("tugas mesin berhenti tiba-tiba")?;
        stats.merge(machine_stats);
        rejected_machines += usize::from(rejected);
        failed_cycles += failures;
    }

    let elapsed = started.elapsed();
    let report = stats.report();
    println!(
        "{} mesin x {} siklus dalam {elapsed:.2?}",
        args.machines, args.cycles
    );
    println!("{report}");
    println!(
        "{:.0} request/detik, {rejected_machines} mesin ditolak server, {failed_cycles} siklus gagal",
        report.requests as f64 / elapsed.as_secs_f64().max(f64::EPSILON)
    );
    if report.errors > 0 || failed_cycles > 0 {
        bail!("ada galat jaringan atau status server");
    }
    Ok(())
}
