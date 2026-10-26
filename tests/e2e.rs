//! Uji end-to-end Task 6: simulator mesin berbicara lewat TCP sungguhan ke server ADMS dan
//! database. Butuh `DATABASE_URL` dengan skema v2.
//!
//! Perilaku firmware yang belum terbukti diuji pada kedua kemungkinan (`Resend::All` seperti yang
//! tercatat di log produksi lama, dan `Resend::FromStamp` bila server mengembalikan stamp).

mod common;

use std::sync::Arc;
use std::time::Instant;

use chrono::{Duration, NaiveDate, NaiveDateTime};
use common::{TestDevice, TestEmployee, app, app_with, default_queue, pool, spawn};
use eabsensi_adms::simulator::{Handshake, Machine, MachineConfig, Record, Resend};
use eabsensi_adms::state::Biometrics;
use eabsensi_adms::vault::Vault;
use sqlx::PgPool;
use zeroize::Zeroizing;

fn base_time() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2022, 3, 14)
        .unwrap()
        .and_hms_opt(7, 0, 0)
        .unwrap()
}

fn records(pin: &str, count: i64, offset_minutes: i64) -> Vec<Record> {
    (0..count)
        .map(|i| Record::new(pin, base_time() + Duration::minutes(offset_minutes + i)))
        .collect()
}

fn machine(authority: &str, device: &TestDevice) -> Machine {
    Machine::new(authority, MachineConfig::new(device.serial.clone()))
}

async fn count(pool: &PgPool, sql: &str, id: i64) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn logs(pool: &PgPool, device: &TestDevice) -> i64 {
    count(
        pool,
        "SELECT count(*) FROM device_logs WHERE device_id = $1",
        device.id,
    )
    .await
}

async fn punches(pool: &PgPool, employee: &TestEmployee) -> i64 {
    count(
        pool,
        "SELECT count(*) FROM attendance_punches WHERE employee_id = $1",
        employee.id,
    )
    .await
}

// ----------------------------------------------------------------- siklus dasar

#[tokio::test]
async fn mesin_baru_menyelesaikan_inisialisasi_lalu_mengirim_ulang_batch_tanpa_duplikat() {
    let pool = pool().await;
    let server = spawn(app(&pool, default_queue())).await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let mut machine = machine(&server.authority(), &device);
    for record in records(&employee.employee_number, 5, 0) {
        machine.add_record(record);
    }

    // Koneksi 1: handshake dasar, unggah 5 log, polling menerima INFO dan menjawabnya.
    let first = machine.cycle().await.unwrap();
    assert!(!first.rejected);
    assert_eq!((first.upload.sent, first.upload.acked), (5, true));
    assert_eq!(first.commands.len(), 1);
    assert!(first.commands[0].text == "INFO");
    assert_eq!(logs(&pool, &device).await, 5);
    assert_eq!(punches(&pool, &employee).await, 5);
    let initialized: bool =
        sqlx::query_scalar("SELECT initialized_at IS NOT NULL FROM devices WHERE id = $1")
            .bind(device.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(initialized, "INFO berhasil menandai mesin terinisialisasi");
    // Alamat klien sampai ke handler lewat penyaji sendiri (`ConnectInfo`).
    let source_ip: Option<String> =
        sqlx::query_scalar("SELECT source_ip FROM device_logs WHERE device_id = $1 LIMIT 1")
            .bind(device.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(source_ip.as_deref(), Some("127.0.0.1"));

    // Koneksi 2: CHECK dari server; handshake kini bentuk "push 2.3.0".
    let second = machine.cycle().await.unwrap();
    assert_eq!(second.commands.len(), 1);
    assert_eq!(second.commands[0].text, "CHECK");
    assert_eq!(device.count_commands(&pool, "check", "succeeded").await, 1);
    let Handshake::Accepted(options) = machine.handshake().await.unwrap() else {
        panic!("mesin terdaftar tidak boleh ditolak");
    };
    assert_eq!(options.get("PushProtVer"), Some("2.3.0"));

    // Koneksi berikutnya mengirim ulang seluruh ingatan (perilaku yang tercatat di log lama).
    for attempt in 3..=6 {
        let report = machine.cycle().await.unwrap();
        assert_eq!(
            (report.upload.sent, report.upload.acked),
            (5, true),
            "koneksi {attempt}"
        );
    }
    assert_eq!(logs(&pool, &device).await, 5, "tidak ada log ganda");
    assert_eq!(punches(&pool, &employee).await, 5, "tidak ada punch ganda");

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn bila_mesin_hanya_mengirim_yang_melewati_stamp_server_siklus_tetap_benar() {
    let pool = pool().await;
    let server = spawn(app(&pool, default_queue())).await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let mut config = MachineConfig::new(device.serial.clone());
    config.resend = Resend::FromStamp;
    let mut machine = Machine::new(server.authority(), config);
    for record in records(&employee.employee_number, 4, 0) {
        machine.add_record(record);
    }

    // Pertama kali: server belum punya stamp, semua dikirim, stamp = log terakhir.
    let first = machine.cycle().await.unwrap();
    assert_eq!(first.upload.sent, 4);

    // Handshake berikutnya mengembalikan stamp itu; tidak ada yang perlu dikirim.
    let second = machine.cycle().await.unwrap();
    assert_eq!((second.upload.sent, second.upload.requests), (0, 0));
    assert!(machine.server_stamp.is_some());

    // Log baru: hanya itu yang dikirim.
    for record in records(&employee.employee_number, 2, 100) {
        machine.add_record(record);
    }
    let third = machine.cycle().await.unwrap();
    assert_eq!((third.upload.sent, third.upload.acked), (2, true));
    assert_eq!(logs(&pool, &device).await, 6);
    assert_eq!(punches(&pool, &employee).await, 6);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn batch_besar_dipecah_per_request_dan_kiriman_ulang_tidak_menambah_apa_pun() {
    let pool = pool().await;
    let server = spawn(app(&pool, default_queue())).await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let mut config = MachineConfig::new(device.serial.clone());
    config.max_rows_per_request = 1000;
    let mut machine = Machine::new(server.authority(), config);
    let rows = 5000;
    for record in records(&employee.employee_number, rows, 0) {
        machine.add_record(record);
    }

    let started = Instant::now();
    let first = machine.upload_records().await.unwrap();
    let first_took = started.elapsed();
    assert_eq!((first.sent, first.requests, first.acked), (5000, 5, true));
    assert_eq!(logs(&pool, &device).await, rows);
    assert_eq!(punches(&pool, &employee).await, rows);

    let started = Instant::now();
    let again = machine.upload_records().await.unwrap();
    let again_took = started.elapsed();
    assert_eq!((again.sent, again.acked), (5000, true));
    assert_eq!(logs(&pool, &device).await, rows);
    assert_eq!(punches(&pool, &employee).await, rows);

    println!("5000 baris lewat HTTP: pertama {first_took:?}, kiriman ulang {again_took:?}");
    assert!(again_took < first_took);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn dua_koneksi_bersamaan_dari_mesin_yang_sama_tidak_membuat_punch_ganda() {
    let pool = pool().await;
    let server = spawn(app(&pool, default_queue())).await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let mut a = machine(&server.authority(), &device);
    let mut b = machine(&server.authority(), &device);
    for record in records(&employee.employee_number, 300, 0) {
        a.add_record(record.clone());
        b.add_record(record);
    }

    // Kasus nyata: mesin mengulang koneksi sebelum koneksi lama selesai.
    let (ra, rb) = tokio::join!(a.upload_records(), b.upload_records());

    assert!(ra.unwrap().acked && rb.unwrap().acked);
    assert_eq!(logs(&pool, &device).await, 300);
    assert_eq!(punches(&pool, &employee).await, 300);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

// -------------------------------------------------------------------- perintah

#[tokio::test]
async fn perintah_operator_dijalankan_mesin_dan_hasilnya_tercatat() {
    let pool = pool().await;
    let server = spawn(app(&pool, default_queue())).await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi Santoso").await;
    let mut machine = machine(&server.authority(), &device);

    // Inisialisasi dulu (INFO lalu CHECK) supaya antrean operator terlihat jelas.
    machine.cycle().await.unwrap();
    machine.cycle().await.unwrap();

    let reboot = device.queue(&pool, "reboot", 1).await;
    let set_time = device.queue(&pool, "set_time", 2).await;
    let add_user = device
        .queue_for(&pool, "add_user", 3, Some(employee.id))
        .await;

    // Satu kelas prioritas per koneksi: tiga koneksi menghabiskan antrean.
    for _ in 0..3 {
        machine.cycle().await.unwrap();
    }

    assert_eq!(machine.reboots, 1);
    assert_eq!(machine.options_set.len(), 2, "zona lalu jam");
    assert!(machine.options_set[0].starts_with("MachineTZ=+0700"));
    assert!(machine.options_set[1].starts_with("DateTime="));
    assert_eq!(machine.users[&employee.employee_number], "Budi Santoso");
    for id in [reboot, set_time, add_user] {
        let row = device.command_row(&pool, id).await;
        assert_eq!(
            row.status, "succeeded",
            "perintah {id}: {:?}",
            row.result_message
        );
        assert_eq!(row.return_code, Some(0));
    }
    assert_eq!(
        machine.cycle().await.unwrap().commands.len(),
        0,
        "antrean habis"
    );

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

// ------------------------------------------------------- pendaftaran mesin (D2)

#[tokio::test]
async fn data_mesin_tak_terdaftar_tidak_dibuang_dan_masuk_setelah_didaftarkan() {
    let pool = pool().await;
    let server = spawn(app(&pool, default_queue())).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let serial = format!("TEST-ADMS-BARU-{}", std::process::id());
    let mut machine = Machine::new(server.authority(), MachineConfig::new(serial.clone()));
    for record in records(&employee.employee_number, 8, 0) {
        machine.add_record(record);
    }

    // Belum terdaftar: handshake ditolak dan unggahan tidak diakui, jadi mesin menyimpan datanya.
    assert!(machine.cycle().await.unwrap().rejected);
    let refused = machine.upload_records().await.unwrap();
    assert!(!refused.acked, "balasan kosong bukan OK");
    assert_eq!(refused.sent, 0);
    assert_eq!(machine.records.len(), 8, "ingatan mesin tidak berkurang");
    assert_eq!(punches(&pool, &employee).await, 0);

    // Operator mendaftarkan mesin; siklus berikutnya mengirim seluruh backlog.
    let device_id: i64 = sqlx::query_scalar(
        "INSERT INTO devices (serial_number, name, created_at, updated_at) \
         VALUES ($1, 'Mesin baru', now(), now()) RETURNING id",
    )
    .bind(&serial)
    .fetch_one(&pool)
    .await
    .unwrap();
    let device = TestDevice {
        id: device_id,
        serial,
    };

    let accepted = machine.cycle().await.unwrap();
    assert!(!accepted.rejected);
    assert_eq!((accepted.upload.sent, accepted.upload.acked), (8, true));
    assert_eq!(punches(&pool, &employee).await, 8);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn mesin_dinonaktifkan_operator_ditolak_lagi() {
    let pool = pool().await;
    let server = spawn(app(&pool, default_queue())).await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let mut machine = machine(&server.authority(), &device);
    for record in records(&employee.employee_number, 3, 0) {
        machine.add_record(record);
    }
    machine.cycle().await.unwrap();
    assert_eq!(punches(&pool, &employee).await, 3);

    device.set(&pool, "is_active = false").await;
    for record in records(&employee.employee_number, 2, 50) {
        machine.add_record(record);
    }
    let report = machine.cycle().await.unwrap();

    assert!(report.rejected);
    assert!(!machine.upload_records().await.unwrap().acked);
    assert_eq!(
        punches(&pool, &employee).await,
        3,
        "tidak ada yang masuk saat nonaktif"
    );

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

// ------------------------------------------------------ biometrik antar mesin

#[tokio::test]
async fn template_dari_satu_mesin_sampai_ke_mesin_lain_lewat_server() {
    let pool = pool().await;
    let vault =
        Arc::new(Vault::new(("k1", &[7u8; 32]), &[] as &[(String, Zeroizing<[u8; 32]>)]).unwrap());
    let biometrics = Biometrics {
        vault: Some(vault),
        store_face_photo: false,
    };
    let server = spawn(app_with(&pool, default_queue(), biometrics)).await;
    let source = TestDevice::create(&pool).await;
    let target = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;

    let mut config = MachineConfig::new(source.serial.clone());
    config.fp_version = Some("10".to_owned());
    let mut a = Machine::new(server.authority(), config);
    let mut config = MachineConfig::new(target.serial.clone());
    config.fp_version = Some("10".to_owned());
    let mut b = Machine::new(server.authority(), config);

    // Keduanya melaporkan INFO (termasuk versi algoritma) sebelum template diunggah/dikirim.
    a.cycle().await.unwrap();
    b.cycle().await.unwrap();

    let template: Vec<u8> = (1..=48).collect();
    let reply = a
        .upload_templates(
            &[(employee.employee_number.clone(), 3, template.clone())],
            99,
        )
        .await
        .unwrap();
    assert!(reply.text().starts_with("OK"));

    let stored: (String, i16, String) = sqlx::query_as(
        "SELECT kind, finger_index, algorithm_version FROM biometric_templates WHERE employee_id = $1",
    )
    .bind(employee.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, ("fingerprint".to_owned(), 3, "10".to_owned()));
    let template_id: i64 =
        sqlx::query_scalar("SELECT id FROM biometric_templates WHERE employee_id = $1")
            .bind(employee.id)
            .fetch_one(&pool)
            .await
            .unwrap();

    let command = sqlx::query_scalar::<_, i64>(
        "INSERT INTO device_commands (device_id, command_type, priority, status, employee_id, \
                biometric_template_id, created_at, updated_at) \
         VALUES ($1, 'add_fingerprint', 4, 'pending', $2, $3, now(), now()) RETURNING id",
    )
    .bind(target.id)
    .bind(employee.id)
    .bind(template_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Kelas prioritas dikirim satu per koneksi: CHECK (kelas 1, menyusul INFO) lebih dulu, lalu
    // pengiriman sidik jari (kelas 4).
    for _ in 0..3 {
        b.cycle().await.unwrap();
    }

    assert_eq!(
        b.templates[&(employee.employee_number.clone(), 3)],
        template,
        "template tiba utuh di mesin tujuan"
    );
    assert_eq!(target.command_row(&pool, command).await.status, "succeeded");

    source.cleanup(&pool).await;
    target.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}
