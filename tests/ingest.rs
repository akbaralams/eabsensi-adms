//! Uji integrasi Task 5: penerimaan ATTLOG (idempotensi, outcome, punch), tabel unggahan lain, dan
//! endpoint pendamping. Butuh `DATABASE_URL` dengan skema v2.
//!
//! Asumsi data: tanggal uji 2024-06 dan 2022-xx tidak berada di periode terkunci pada database
//! uji; uji yang membutuhkan periode membuatnya sendiri dengan bulan khusus (2021-03..2021-05).

mod common;

use std::time::Instant;

use axum::http::{Method, StatusCode};
use chrono::{Duration, NaiveDate, Utc};
use common::{TestDevice, TestEmployee, TestPeriod, app, default_queue, get, pool, post, send};
use sqlx::PgPool;

type LogRow = (String, Option<i64>, Option<i64>, bool, String);

/// push_version, jumlah user, jumlah jari, jumlah log, model, alamat IP.
type OptionsRow = (
    Option<String>,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    Option<String>,
    Option<String>,
);

fn attlog_uri(device: &TestDevice, stamp: u64) -> String {
    format!(
        "/iclock/cdata?SN={}&table=ATTLOG&Stamp={stamp}",
        device.serial
    )
}

async fn count_logs(pool: &PgPool, device: &TestDevice) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM device_logs WHERE device_id = $1")
        .bind(device.id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count_punches(pool: &PgPool, employee: &TestEmployee) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM attendance_punches WHERE employee_id = $1")
        .bind(employee.id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// outcome, employee_id, punch_id, is_suspect, punched_at (UTC, teks) untuk satu PIN.
async fn log_of(pool: &PgPool, device: &TestDevice, pin: &str, device_time: &str) -> LogRow {
    sqlx::query_as(
        "SELECT outcome, employee_id, punch_id, is_suspect, \
                to_char(punched_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS') \
           FROM device_logs \
          WHERE device_id = $1 AND pin = $2 AND device_time = $3::timestamp",
    )
    .bind(device.id)
    .bind(pin)
    .bind(device_time)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn stamp_of(pool: &PgPool, device: &TestDevice, column: &str) -> Option<String> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT {column} FROM devices WHERE id = $1"
    )))
    .bind(device.id)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ------------------------------------------------------------------ ATTLOG

#[tokio::test]
async fn attlog_menyimpan_log_mentah_dan_membuat_punch_approved_dari_mesin() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue());

    let reply = post(
        &app,
        &attlog_uri(&device, 785_168_153),
        &format!("{pin}\t2024-06-05 07:01:03\t0\t1\t0\t0\t0\n"),
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.content_type, "text/plain; charset=UTF-8");
    assert_eq!(reply.body, "OK:1\n");

    let log = log_of(&pool, &device, pin, "2024-06-05 07:01:03").await;
    assert_eq!(log.0, "punched");
    assert_eq!(log.1, Some(employee.id));
    assert!(log.2.is_some(), "log harus menaut ke punch");
    assert!(!log.3, "bukan suspect");
    assert_eq!(
        log.4, "2024-06-05 00:01:03",
        "WIB (UTC+7) dikonversi ke UTC"
    );

    let punch: (String, String, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT source, status, created_by, photo_path FROM attendance_punches WHERE id = $1",
    )
    .bind(log.2.unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        punch,
        ("device".to_owned(), "approved".to_owned(), None, None)
    );

    let io: (Option<i16>, Option<i16>) =
        sqlx::query_as("SELECT io_status, verify_mode FROM device_logs WHERE device_id = $1")
            .bind(device.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(io, (Some(0), Some(1)));

    // Stamp disimpan persis seperti dikirim mesin dan dikembalikan di handshake berikutnya.
    assert_eq!(
        stamp_of(&pool, &device, "attlog_stamp").await.as_deref(),
        Some("785168153")
    );
    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn kiriman_ulang_batch_yang_sama_tidak_menambah_log_maupun_punch() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue());
    let body = format!("{pin}\t2024-06-05 07:01:03\t0\t1\n{pin}\t2024-06-05 16:02:00\t1\t1\n");

    for attempt in 1..=3 {
        let reply = post(&app, &attlog_uri(&device, 100), &body).await;
        assert_eq!(reply.body, "OK:2\n", "kiriman ke-{attempt} tetap diakui");
    }

    assert_eq!(count_logs(&pool, &device).await, 2);
    assert_eq!(count_punches(&pool, &employee).await, 2);

    // Batch yang bertambah satu baris: hanya baris baru yang masuk.
    let longer = format!("{body}{pin}\t2024-06-06 07:00:00\t0\t1\n");
    assert_eq!(
        post(&app, &attlog_uri(&device, 200), &longer).await.body,
        "OK:3\n"
    );
    assert_eq!(count_logs(&pool, &device).await, 3);
    assert_eq!(count_punches(&pool, &employee).await, 3);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn baris_kembar_dalam_satu_batch_hanya_dihitung_sekali() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue());
    let row = format!("{pin}\t2024-06-05 07:01:03\t0\t1\n");

    let reply = post(&app, &attlog_uri(&device, 1), &format!("{row}{row}")).await;

    assert_eq!(reply.body, "OK:2\n");
    assert_eq!(count_logs(&pool, &device).await, 1);
    assert_eq!(count_punches(&pool, &employee).await, 1);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn setiap_outcome_disimpan_dan_hanya_yang_punched_membuat_punch() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let other = TestDevice::create(&pool).await;
    let active = TestEmployee::create(&pool, "Aktif").await;
    let inactive = TestEmployee::create(&pool, "Nonaktif").await;
    inactive.set_active(&pool, false).await;
    let in_closed = TestEmployee::create(&pool, "Periode Tutup").await;
    let dupe = TestEmployee::create(&pool, "Duplikat").await;
    // Bulan khusus uji ini: Maret 2021 ditutup, April 2021 sedang ditutup, Mei 2021 dibuka lagi.
    let closed = TestPeriod::create(&pool, 2021, 3, "closed").await;
    let closing = TestPeriod::create(&pool, 2021, 4, "closing").await;
    let reopened = TestPeriod::create(&pool, 2021, 5, "open").await;
    let app = app(&pool, default_queue());

    // Punch approved dari mesin lain pada detik yang sama => log ini menjadi duplicate.
    sqlx::query(
        "INSERT INTO attendance_punches (employee_id, punched_at, source, status, created_at, updated_at) \
         VALUES ($1, '2022-02-02 00:55:00+00', 'device', 'approved', now(), now())",
    )
    .bind(dupe.id)
    .execute(&pool)
    .await
    .unwrap();

    let body = [
        format!("{}\t2022-02-02 07:55:00\t0\t1", active.employee_number),
        "TIDAKADA\t2022-02-02 07:55:00\t0\t1".to_owned(),
        format!("{}\t2022-02-02 07:55:00\t0\t1", inactive.employee_number),
        format!("{}\t2021-03-10 07:55:00\t0\t1", in_closed.employee_number),
        format!("{}\t2021-04-10 07:55:00\t0\t1", in_closed.employee_number),
        format!("{}\t2021-05-10 07:55:00\t0\t1", in_closed.employee_number),
        format!("{}\t2022-02-02 07:55:00\t0\t1", dupe.employee_number),
        format!("{}\t2019-12-31 23:59:59\t0\t1", active.employee_number),
    ]
    .join("\n");

    let reply = post(&app, &attlog_uri(&device, 5), &body).await;
    assert_eq!(reply.body, "OK:8\n");

    let expect = [
        (
            active.employee_number.as_str(),
            "2022-02-02 07:55:00",
            "punched",
            Some(active.id),
            true,
        ),
        (
            "TIDAKADA",
            "2022-02-02 07:55:00",
            "unknown_pin",
            None,
            false,
        ),
        (
            inactive.employee_number.as_str(),
            "2022-02-02 07:55:00",
            "employee_inactive",
            Some(inactive.id),
            false,
        ),
        (
            in_closed.employee_number.as_str(),
            "2021-03-10 07:55:00",
            "period_closed",
            Some(in_closed.id),
            false,
        ),
        (
            in_closed.employee_number.as_str(),
            "2021-04-10 07:55:00",
            "period_closed",
            Some(in_closed.id),
            false,
        ),
        (
            in_closed.employee_number.as_str(),
            "2021-05-10 07:55:00",
            "punched",
            Some(in_closed.id),
            true,
        ),
        (
            dupe.employee_number.as_str(),
            "2022-02-02 07:55:00",
            "duplicate",
            Some(dupe.id),
            false,
        ),
        (
            active.employee_number.as_str(),
            "2019-12-31 23:59:59",
            "out_of_range",
            None,
            false,
        ),
    ];
    for (pin, time, outcome, employee_id, has_punch) in expect {
        let log = log_of(&pool, &device, pin, time).await;
        assert_eq!(log.0, outcome, "{pin} {time}");
        assert_eq!(log.1, employee_id, "employee_id {pin} {time}");
        assert_eq!(log.2.is_some(), has_punch, "punch_id {pin} {time}");
    }

    // Punch: aktif 1, periode dibuka 1, duplikat tetap 1 (yang lama), lainnya 0.
    assert_eq!(count_punches(&pool, &active).await, 1);
    assert_eq!(count_punches(&pool, &inactive).await, 0);
    assert_eq!(count_punches(&pool, &in_closed).await, 1);
    assert_eq!(count_punches(&pool, &dupe).await, 1);

    // Setelah periode dibuka, log yang sama dikirim ulang tetap tidak berubah (log mentah sudah
    // ada; pemrosesan ulang adalah tugas UI v2).
    closed.cleanup(&pool).await;
    closing.cleanup(&pool).await;
    reopened.cleanup(&pool).await;
    assert_eq!(
        post(&app, &attlog_uri(&device, 6), &body).await.body,
        "OK:8\n"
    );
    assert_eq!(count_punches(&pool, &in_closed).await, 1);
    assert_eq!(
        log_of(
            &pool,
            &device,
            &in_closed.employee_number,
            "2021-03-10 07:55:00"
        )
        .await
        .0,
        "period_closed"
    );

    device.cleanup(&pool).await;
    other.cleanup(&pool).await;
    for e in [active, inactive, in_closed, dupe] {
        e.cleanup(&pool).await;
    }
}

#[tokio::test]
async fn pin_dicocokkan_persis_tanpa_trim_dan_tanpa_padding() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = employee.employee_number.clone();
    let app = app(&pool, default_queue());

    let body = format!(
        "0{pin}\t2024-06-05 07:00:00\t0\t1\n{pin} \t2024-06-05 07:00:01\t0\t1\n{pin}\t2024-06-05 07:00:02\t0\t1\n"
    );
    post(&app, &attlog_uri(&device, 1), &body).await;

    assert_eq!(
        log_of(&pool, &device, &format!("0{pin}"), "2024-06-05 07:00:00")
            .await
            .0,
        "unknown_pin"
    );
    assert_eq!(
        log_of(&pool, &device, &format!("{pin} "), "2024-06-05 07:00:01")
            .await
            .0,
        "unknown_pin"
    );
    assert_eq!(
        log_of(&pool, &device, &pin, "2024-06-05 07:00:02").await.0,
        "punched"
    );
    assert_eq!(count_punches(&pool, &employee).await, 1);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn log_di_masa_depan_ditandai_suspect_tetapi_tetap_menjadi_punch() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue());

    // Waktu lokal Jakarta satu jam di depan, dan satu menit di depan (di bawah ambang 500 detik).
    let jakarta = Utc::now() + Duration::hours(7);
    let future = (jakarta + Duration::hours(1))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let near = (jakarta + Duration::seconds(60))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let body = format!("{pin}\t{future}\t0\t1\n{pin}\t{near}\t0\t1\n");

    post(&app, &attlog_uri(&device, 1), &body).await;

    let far = log_of(&pool, &device, pin, &future).await;
    assert_eq!(far.0, "punched", "D4b: suspect tetap punch approved");
    assert!(far.3, "satu jam di depan harus suspect");
    assert!(far.2.is_some());
    let close = log_of(&pool, &device, pin, &near).await;
    assert!(!close.3, "satu menit di depan masih wajar");

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn zona_waktu_mesin_dipakai_untuk_konversi_ke_utc() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    device.set(&pool, "timezone = 'Asia/Makassar'").await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue());

    post(
        &app,
        &attlog_uri(&device, 1),
        &format!("{pin}\t2024-06-05 08:00:00\t0\t1\n"),
    )
    .await;

    assert_eq!(
        log_of(&pool, &device, pin, "2024-06-05 08:00:00").await.4,
        "2024-06-05 00:00:00"
    );

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn baris_rusak_dilewati_dan_sisanya_diproses_dengan_hitungan_penuh() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue());
    let body = format!(
        "{pin}\n\t2024-06-05 07:00:00\n{pin}\tkemarin\n{pin}\t2024-06-05 07:00:03\t0\t1\r\n\r\n{}\t2024-06-05 07:00:04\n",
        "9".repeat(40)
    );

    let reply = post(&app, &attlog_uri(&device, 1), &body).await;

    // 5 baris tak kosong: 4 rusak (tanpa waktu, PIN kosong, waktu salah, PIN terlalu panjang)
    // dan 1 sah; baris yang hanya berisi CR tidak dihitung.
    assert_eq!(reply.body, "OK:5\n");
    assert_eq!(count_logs(&pool, &device).await, 1);
    assert_eq!(count_punches(&pool, &employee).await, 1);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn badan_kosong_dibalas_ok_dan_tetap_memajukan_stamp() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());

    let reply = post(&app, &attlog_uri(&device, 777), "").await;

    assert_eq!(reply.body, "OK\n");
    assert_eq!(
        stamp_of(&pool, &device, "attlog_stamp").await.as_deref(),
        Some("777")
    );
    assert_eq!(count_logs(&pool, &device).await, 0);

    // Stamp tidak valid diabaikan dan tidak merusak stamp lama.
    let bad = format!(
        "/iclock/cdata?SN={}&table=ATTLOG&Stamp=abc;drop",
        device.serial
    );
    assert_eq!(post(&app, &bad, "").await.body, "OK\n");
    assert_eq!(
        stamp_of(&pool, &device, "attlog_stamp").await.as_deref(),
        Some("777")
    );

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn stamp_tanpa_table_dianggap_attlog() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue());

    let reply = post(
        &app,
        &format!("/iclock/cdata?SN={}&Stamp=55", device.serial),
        &format!("{pin}\t2024-06-05 07:00:00\t0\t1\n"),
    )
    .await;

    assert_eq!(reply.body, "OK:1\n");
    assert_eq!(count_punches(&pool, &employee).await, 1);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn unggahan_mesin_tak_terdaftar_atau_nonaktif_ditolak_tanpa_ok_dan_tanpa_efek() {
    let pool = pool().await;
    let inactive = TestDevice::create_inactive(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue());
    let body = format!("{pin}\t2024-06-05 07:00:00\t0\t1\n");

    let unknown = post(
        &app,
        "/iclock/cdata?SN=TIDAK-TERDAFTAR-9&table=ATTLOG&Stamp=1",
        &body,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::OK);
    assert_eq!(
        unknown.body, "",
        "bukan OK: mesin akan menyimpan datanya dan mengirim ulang"
    );

    let reply = post(&app, &attlog_uri(&inactive, 1), &body).await;
    assert_eq!(reply.body, "");
    assert_eq!(count_logs(&pool, &inactive).await, 0);
    assert_eq!(count_punches(&pool, &employee).await, 0);
    assert_eq!(stamp_of(&pool, &inactive, "attlog_stamp").await, None);
    assert_eq!(inactive.seen(&pool).await, (false, false));

    // Bukan request mesin (User-Agent lain) jatuh ke balasan akhir `OK` tanpa efek.
    let browser = send(
        &app,
        Method::POST,
        &attlog_uri(&inactive, 1),
        &body,
        Some("Mozilla/5.0"),
    )
    .await;
    assert_eq!(browser.body, "OK\n");

    inactive.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn unggahan_sah_mencatat_heartbeat() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());

    post(&app, &attlog_uri(&device, 1), "").await;

    assert_eq!(device.seen(&pool).await, (true, true));
    device.cleanup(&pool).await;
}

// ----------------------------------------------------------- tabel unggahan lain

#[tokio::test]
async fn tabel_lain_dibalas_sesuai_keputusan_dan_stamp_disimpan_apa_adanya() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue());
    let sn = &device.serial;

    // rtstate, rtlog, tabledata: diabaikan (D7), balasan seperti kode lama.
    assert_eq!(
        post(&app, &format!("/iclock/cdata?SN={sn}&table=rtstate"), "x=1")
            .await
            .body,
        "OK"
    );
    assert_eq!(
        post(
            &app,
            &format!("/iclock/cdata?SN={sn}&table=rtlog"),
            &format!("time=2024-06-05 07:00:00\tpin={pin}\tinoutstatus=0\n")
        )
        .await
        .body,
        "OK"
    );
    assert_eq!(
        post(
            &app,
            &format!("/iclock/cdata?SN={sn}&table=tabledata&tablename=user&count=3"),
            "user x"
        )
        .await
        .body,
        "OK\n"
    );

    // OPERLOG: hanya stamp, jumlah baris diakui, tidak ada log atau punch (D6).
    let operlog = post(
        &app,
        &format!("/iclock/cdata?SN={sn}&table=OPERLOG&OpStamp=785154440"),
        "OPLOG 4\t0\t2024-06-05 10:27:20\t0\t0\t0\t0\nFP PIN=1\tFID=0\tSize=1\tValid=1\tTMP=AAA\n\n",
    )
    .await;
    assert_eq!(operlog.body, "OK:2\n");
    assert_eq!(
        stamp_of(&pool, &device, "operlog_stamp").await.as_deref(),
        Some("785154440")
    );

    // BIODATA dan ATTPHOTO: hanya stamp.
    assert_eq!(
        post(
            &app,
            &format!("/iclock/cdata?SN={sn}&table=BIODATA&Stamp=991"),
            "BIODATA x"
        )
        .await
        .body,
        "OK\n"
    );
    assert_eq!(
        stamp_of(&pool, &device, "biodata_stamp").await.as_deref(),
        Some("991")
    );
    assert_eq!(
        post(
            &app,
            &format!("/iclock/cdata?SN={sn}&table=ATTPHOTO&PhotoStamp=992"),
            "pin=1.jpg"
        )
        .await
        .body,
        "OK\n"
    );
    assert_eq!(
        stamp_of(&pool, &device, "photo_stamp").await.as_deref(),
        Some("992")
    );

    assert_eq!(count_logs(&pool, &device).await, 0);
    assert_eq!(count_punches(&pool, &employee).await, 0);

    // Tanpa table dan tanpa stamp: balasan akhir OK.
    assert_eq!(
        post(&app, &format!("/iclock/cdata?SN={sn}"), "").await.body,
        "OK\n"
    );

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn table_options_memperbarui_info_mesin() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());

    let reply = post(
        &app,
        &format!("/iclock/cdata?SN={}&table=options", device.serial),
        "PushVersion=2.1.44,UserCount=12,FPCount=20,FaceCount=0,TransactionCount=345,~DeviceName=SpeedFace,~Platform=ZMM220,IPAddress=10.0.0.9",
    )
    .await;

    assert_eq!(reply.body, "OK");
    let row: OptionsRow = sqlx::query_as(
        "SELECT push_version, user_count, fingerprint_count, log_count, model, ip_address \
               FROM devices WHERE id = $1",
    )
    .bind(device.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0.as_deref(), Some("2.1.44"));
    assert_eq!((row.1, row.2, row.3), (Some(12), Some(20), Some(345)));
    assert_eq!(row.4.as_deref(), Some("SpeedFace"));
    assert_eq!(row.5.as_deref(), Some("10.0.0.9"));

    device.cleanup(&pool).await;
}

// ---------------------------------------------------------- endpoint pendamping

#[tokio::test]
async fn fdata_foto_diabaikan_dengan_balasan_ok() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let inactive = TestDevice::create_inactive(&pool).await;
    let app = app(&pool, default_queue());

    assert_eq!(
        post(&app, &format!("/iclock/fdata?SN={}", device.serial), "JPEG")
            .await
            .body,
        "OK\n"
    );
    assert_eq!(device.seen(&pool).await, (true, true));
    assert_eq!(
        post(
            &app,
            &format!("/iclock/fdata?SN={}", inactive.serial),
            "JPEG"
        )
        .await
        .body,
        ""
    );
    assert_eq!(
        post(&app, "/iclock/fdata?SN=TIDAK-ADA-1", "JPEG")
            .await
            .body,
        ""
    );

    device.cleanup(&pool).await;
    inactive.cleanup(&pool).await;
}

#[tokio::test]
async fn registry_memberi_kode_tetap_dan_menyimpan_tipe_mesin() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());
    let uri = format!("/iclock/registry?SN={}", device.serial);

    let first = post(
        &app,
        &uri,
        "DeviceType=1,PushVersion=2.1.44,UserCount=4,~DeviceName=InBio",
    )
    .await;

    let code = first
        .body
        .strip_prefix("RegistryCode=")
        .expect("awalan RegistryCode=");
    assert_eq!(code.len(), 10);
    assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
    assert_eq!(first.content_type, "text/plain; charset=UTF-8");

    let second = post(&app, &uri, "DeviceType=att").await;
    assert_eq!(second.body, first.body, "kode registrasi tidak berubah");
    let row: (String, Option<String>, Option<i32>) =
        sqlx::query_as("SELECT device_type, model, user_count FROM devices WHERE id = $1")
            .bind(device.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "att");
    assert_eq!(
        row.1.as_deref(),
        Some("InBio"),
        "tidak ditimpa bila tak dilaporkan"
    );
    assert_eq!(row.2, Some(4));

    assert_eq!(
        post(&app, "/iclock/registry?SN=TIDAK-ADA-2", "DeviceType=1")
            .await
            .body,
        "UNKNOWN DEVICE"
    );

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn push_membalas_parameter_sesi_dan_meminta_set_time_sekali() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    device
        .set(
            &pool,
            "push_version = '2.1.44', poll_delay_seconds = 30, error_delay_seconds = 90",
        )
        .await;
    let app = app(&pool, default_queue());
    let uri = format!("/iclock/push?SN={}", device.serial);

    let reply = post(&app, &uri, "").await;

    let lines: Vec<&str> = reply.body.split('\n').collect();
    assert_eq!(lines.len(), 10, "tanpa LF akhir");
    assert_eq!(
        &lines[..5],
        [
            "ServerVersion=2.1.44",
            "ServerName=ADMS",
            "PushVersion=2.1.44",
            "ErrorDelay=90",
            "RequestDelay=30",
        ]
    );
    assert_eq!(lines[5], "TransTimes=00:00\t14:00");
    assert_eq!(lines[7], "TransTables=User\tTransaction");
    assert!(lines[8].starts_with("SessionID=") && lines[8].len() == "SessionID=".len() + 10);
    assert_eq!(lines[9], "TimeoutSec=10");

    post(&app, &uri, "").await;
    assert_eq!(device.count_commands(&pool, "set_time", "pending").await, 1);
    assert_eq!(post(&app, "/iclock/push?SN=TIDAK-ADA-3", "").await.body, "");

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn rtdata_time_membalas_jam_server_dan_zona_mesin() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    device.set(&pool, "timezone = 'Asia/Jayapura'").await;
    let app = app(&pool, default_queue());

    let reply = get(
        &app,
        &format!("/iclock/rtdata?SN={}&type=time", device.serial),
    )
    .await;

    let rest = reply.body.strip_prefix("DateTime=").expect("DateTime=");
    let (seconds, zone) = rest.split_once(",ServerTZ=").expect("ServerTZ");
    assert!(seconds.parse::<i64>().unwrap() > 785_000_000);
    assert_eq!(zone, "+0900");

    assert_eq!(
        get(
            &app,
            &format!("/iclock/rtdata?SN={}&type=lain", device.serial)
        )
        .await
        .body,
        ""
    );
    assert_eq!(
        get(&app, "/iclock/rtdata?SN=TIDAK-ADA-4&type=time")
            .await
            .body,
        ""
    );

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn ping_selalu_ok_tanpa_pemeriksaan() {
    let pool = pool().await;
    let app = app(&pool, default_queue());

    let reply = get(&app, "/iclock/ping").await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body, "OK");
    assert_eq!(reply.content_type, "text/plain; charset=UTF-8");
}

#[tokio::test]
async fn querydata_options_count_dan_tabledata() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());
    let sn = &device.serial;

    assert_eq!(
        post(&app, &format!("/iclock/querydata?SN={sn}&type=options"), "")
            .await
            .body,
        "OK"
    );

    assert_eq!(
        post(
            &app,
            &format!("/iclock/querydata?SN={sn}&type=count&tablename=user"),
            "user=12\ntemplatev10=20\nbiophoto=3"
        )
        .await
        .body,
        "OK"
    );
    let counts: (Option<i32>, Option<i32>, Option<i32>) = sqlx::query_as(
        "SELECT user_count, fingerprint_count, face_count FROM devices WHERE id = $1",
    )
    .bind(device.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (Some(12), Some(20), Some(3)));

    // tabledata hanya memantulkan nama dan jumlah (A1), tanpa menyimpan data.
    assert_eq!(
        post(
            &app,
            &format!("/iclock/querydata?SN={sn}&type=tabledata&tablename=transaction&count=5"),
            "pin=1\ttime_second=1"
        )
        .await
        .body,
        "transaction=5"
    );
    assert_eq!(count_logs(&pool, &device).await, 0);

    // Nama berisi karakter berbahaya tidak dipantulkan.
    assert_eq!(
        post(
            &app,
            &format!("/iclock/querydata?SN={sn}&type=tabledata&tablename=a%0Ab&count=5"),
            ""
        )
        .await
        .body,
        ""
    );
    assert_eq!(
        post(&app, &format!("/iclock/querydata?SN={sn}&type=aneh"), "")
            .await
            .body,
        ""
    );
    assert_eq!(
        post(&app, "/iclock/querydata?SN=TIDAK-ADA-5&type=options", "")
            .await
            .body,
        ""
    );

    device.cleanup(&pool).await;
}

// ------------------------------------------------------------------- beban

#[tokio::test]
async fn batch_besar_diterima_dalam_satu_transaksi_dan_kiriman_ulang_murah() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue());

    let start = NaiveDate::from_ymd_opt(2023, 1, 2)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let rows = 2000;
    let body: String = (0..rows)
        .map(|i| {
            let time = start + Duration::minutes(i);
            format!("{pin}\t{}\t0\t1\n", time.format("%Y-%m-%d %H:%M:%S"))
        })
        .collect();
    let uri = attlog_uri(&device, 9);

    let first_start = Instant::now();
    let first = post(&app, &uri, &body).await;
    let first_took = first_start.elapsed();
    assert_eq!(first.body, format!("OK:{rows}\n"));
    assert_eq!(count_logs(&pool, &device).await, rows);
    assert_eq!(count_punches(&pool, &employee).await, rows);

    let resend_start = Instant::now();
    let resend = post(&app, &uri, &body).await;
    let resend_took = resend_start.elapsed();
    assert_eq!(resend.body, format!("OK:{rows}\n"));
    assert_eq!(count_logs(&pool, &device).await, rows);
    assert_eq!(count_punches(&pool, &employee).await, rows);

    println!("2000 baris: pertama {first_took:?}, kiriman ulang {resend_took:?}");
    assert!(
        first_took.as_secs() < 25,
        "batch pertama terlalu lambat: {first_took:?}"
    );
    assert!(
        resend_took < first_took,
        "kiriman ulang harus lebih murah dari batch baru"
    );

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}
