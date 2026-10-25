//! Uji integrasi Task 4: handshake, getrequest, devicecmd, heartbeat, dan perlakuan mesin tak
//! dikenal. Butuh `DATABASE_URL` dengan skema v2 (tabel mesin).

mod common;

use axum::http::{Method, StatusCode};
use common::{TestDevice, TestEmployee, app, default_queue, get, pool, post, send};
use eabsensi_adms::state::QueueSettings;
use std::time::Duration;

/// firmware, jumlah user, jumlah jari, jumlah log, alamat IP.
type ReportedRow = (
    Option<String>,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    Option<String>,
);

/// terinisialisasi, versi push, jumlah user, jumlah log, model, `reported_options.PushVersion`.
type InitializedRow = (
    bool,
    Option<String>,
    Option<i32>,
    Option<i32>,
    Option<String>,
    Option<String>,
);

// ---------------------------------------------------------------- handshake

#[tokio::test]
async fn handshake_mesin_baru_bentuk_dasar_dan_meminta_info() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());
    let uri = format!(
        "/iclock/cdata?SN={}&options=all&pushver=2.1.44&language=69",
        device.serial
    );

    let reply = get(&app, &uri).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.content_type, "text/plain; charset=UTF-8");
    assert_eq!(
        reply.body,
        format!(
            "GET OPTION FROM:{}\nStamp=None\nOpStamp=None\nPhotoStamp=None\nErrorDelay=60\nDelay=60\n\
             TransTimes=00:00;14:05\nTransInterval=1\nTransFlag=111111111111\nTimeZone=7\n\
             Realtime=1\nEncrypt=0\n",
            device.serial
        )
    );
    // Heartbeat tercatat dan INFO diantrekan satu kali, juga pada handshake berikutnya.
    assert_eq!(device.seen(&pool).await, (true, true));
    assert_eq!(device.count_commands(&pool, "info", "pending").await, 1);
    get(&app, &uri).await;
    assert_eq!(device.count_commands(&pool, "info", "pending").await, 1);

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn handshake_mesin_baru_bertipe_att() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());

    let reply = get(
        &app,
        &format!(
            "/iclock/cdata?SN={}&options=all&DeviceType=att",
            device.serial
        ),
    )
    .await;

    assert!(
        reply.body.contains(
            "ATTLOGStamp=None\nOPERLOGStamp=None\nBIODATAStamp=None\nATTPHOTOStamp=None\n"
        )
    );
    assert!(reply.body.contains("TransFlag=111110101101\n"));
    assert!(
        reply
            .body
            .ends_with("MultiBioPhotoSupport=0:0:0:0:0:0:0:0:0:1\n")
    );
    let stored: String = sqlx::query_scalar("SELECT device_type FROM devices WHERE id = $1")
        .bind(device.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, "att");

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn handshake_push3_membalas_ok_dan_menyimpan_versi() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());

    let reply = get(
        &app,
        &format!(
            "/iclock/cdata?SN={}&options=all&pushver=3.0.1&PushOptionsFlag=1",
            device.serial
        ),
    )
    .await;

    assert_eq!(reply.body, "OK\nPushProtVer=3.0.1\nPushOptionsFlag=1");
    let stored: Option<String> =
        sqlx::query_scalar("SELECT push_version FROM devices WHERE id = $1")
            .bind(device.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some("3.0.1"));
    // Jalur push 3.x tidak membuat perintah INFO (sama dengan kode lama).
    assert_eq!(device.count_commands(&pool, "info", "pending").await, 0);

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn handshake_mesin_lama_dengan_versi_push_memakai_stamp_tersimpan() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    device
        .set(
            &pool,
            "initialized_at = now(), push_version = '2.1.44', attlog_stamp = '785168153', \
             operlog_stamp = '785154440', timezone = 'Asia/Makassar', poll_delay_seconds = 30, \
             error_delay_seconds = 90",
        )
        .await;
    let app = app(&pool, default_queue());

    let reply = get(
        &app,
        &format!("/iclock/cdata?SN={}&options=all", device.serial),
    )
    .await;

    assert_eq!(
        reply.body,
        format!(
            "GET OPTION FROM:{}\nStamp=785168153\nOpStamp=785154440\nPhotoStamp=None\nErrorDelay=90\nDelay=30\n\
             PushProtVer=2.3.0\nServerVer=2.3.0\nEncryptFlag=1000000000\nTransTimes=00:00;14:05\n\
             TransInterval=1\nTransFlag=TransData AttLog\tOpLog\tAttPhoto\tEnrollFP\tEnrollUser\tFPImag\tChgUser\tChgFP\tFACE\tUserPic\tFVEIN\tBioPhoto\n\
             TimeZone=8\nRealtime=1\nEncrypt=0\n",
            device.serial
        )
    );
    // Sudah terinisialisasi: tidak meminta INFO lagi.
    assert_eq!(device.count_commands(&pool, "info", "pending").await, 0);

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn handshake_ditolak_tanpa_efek_untuk_mesin_tak_dikenal_nonaktif_dan_request_cacat() {
    let pool = pool().await;
    let inactive = TestDevice::create_inactive(&pool).await;
    let active = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());

    // Tak dikenal.
    let unknown = get(&app, "/iclock/cdata?SN=TIDAK-TERDAFTAR-1&options=all").await;
    assert_eq!(unknown.status, StatusCode::OK);
    assert_eq!(unknown.body, "");

    // Nonaktif: kosong, tidak ada heartbeat dan tidak ada perintah.
    let reply = get(
        &app,
        &format!("/iclock/cdata?SN={}&options=all", inactive.serial),
    )
    .await;
    assert_eq!(reply.body, "");
    assert_eq!(inactive.seen(&pool).await, (false, false));
    assert_eq!(inactive.count_commands(&pool, "info", "pending").await, 0);

    // User-Agent bukan mesin, tanpa options, tanpa SN: semuanya kosong dan tanpa efek.
    let no_agent = send(
        &app,
        Method::GET,
        &format!("/iclock/cdata?SN={}&options=all", active.serial),
        "",
        Some("curl/8.0"),
    )
    .await;
    assert_eq!(no_agent.body, "");
    let no_options = get(&app, &format!("/iclock/cdata?SN={}", active.serial)).await;
    assert_eq!(no_options.body, "");
    let no_sn = get(&app, "/iclock/cdata?options=all").await;
    assert_eq!(no_sn.body, "");
    assert_eq!(active.seen(&pool).await, (false, false));

    inactive.cleanup(&pool).await;
    active.cleanup(&pool).await;
}

// ---------------------------------------------------------------- getrequest

#[tokio::test]
async fn getrequest_tanpa_perintah_membalas_ok_dan_mencatat_heartbeat() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());

    let reply = get(&app, &format!("/iclock/getrequest?SN={}", device.serial)).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.content_type, "text/plain; charset=UTF-8");
    assert_eq!(reply.body, "OK");
    assert_eq!(device.seen(&pool).await, (true, true));

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn getrequest_mesin_tak_dikenal_atau_nonaktif_dijawab_ok_tanpa_efek() {
    let pool = pool().await;
    let inactive = TestDevice::create_inactive(&pool).await;
    inactive.queue(&pool, "check", 1).await;
    let app = app(&pool, default_queue());

    let unknown = get(&app, "/iclock/getrequest?SN=TIDAK-TERDAFTAR-2").await;
    assert_eq!(unknown.body, "OK");

    let reply = get(&app, &format!("/iclock/getrequest?SN={}", inactive.serial)).await;
    assert_eq!(reply.body, "OK");
    assert_eq!(inactive.seen(&pool).await, (false, false));
    assert_eq!(inactive.count_commands(&pool, "check", "pending").await, 1);

    inactive.cleanup(&pool).await;
}

#[tokio::test]
async fn getrequest_dengan_info_memperbarui_data_mesin() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());

    let reply = get(
        &app,
        &format!(
            "/iclock/getrequest?SN={}&INFO=Ver%206.60%20Apr%2028%202019,12,20,345,192.168.1.50",
            device.serial
        ),
    )
    .await;

    assert_eq!(reply.body, "OK");
    let row: ReportedRow = sqlx::query_as(
        "SELECT firmware_version, user_count, fingerprint_count, log_count, ip_address \
               FROM devices WHERE id = $1",
    )
    .bind(device.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0.as_deref(), Some("Ver 6.60 Apr 28 2019"));
    assert_eq!((row.1, row.2, row.3), (Some(12), Some(20), Some(345)));
    assert_eq!(row.4.as_deref(), Some("192.168.1.50"));

    // Laporan yang pendek tidak menjadi galat.
    let short = get(
        &app,
        &format!("/iclock/getrequest?SN={}&INFO=Ver%201.0", device.serial),
    )
    .await;
    assert_eq!(short.status, StatusCode::OK);

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn getrequest_mengirim_satu_kelas_prioritas_per_poll_sesuai_batas_lama() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi\tSantoso").await;
    let app = app(&pool, default_queue());
    let uri = format!("/iclock/getrequest?SN={}", device.serial);

    // Didaftarkan dalam urutan terbalik dari prioritasnya.
    let add_user = device
        .queue_for(&pool, "add_user", 3, Some(employee.id))
        .await;
    let set_time = device.queue(&pool, "set_time", 2).await;
    let reboot = device.queue(&pool, "reboot", 1).await;
    let check = device.queue(&pool, "check", 1).await;

    // Kelas 1: hanya satu perintah per poll, menurut id.
    let first = get(&app, &uri).await;
    assert_eq!(first.body, format!("C:{reboot}:REBOOT\n"));
    assert_eq!(device.command_row(&pool, reboot).await.status, "sent");
    assert_eq!(device.command_row(&pool, reboot).await.attempts, 1);

    let second = get(&app, &uri).await;
    assert_eq!(second.body, format!("C:{check}:CHECK\n"));

    // Kelas 2: set_time menjadi dua baris dengan id yang sama.
    let third = get(&app, &uri).await;
    let lines: Vec<&str> = third.body.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0],
        format!("C:{set_time}:SET OPTIONS MachineTZ=+0700")
    );
    assert!(lines[1].starts_with(&format!("C:{set_time}:SET OPTIONS DateTime=")));
    assert!(
        lines[1]
            .trim_start_matches(&format!("C:{set_time}:SET OPTIONS DateTime="))
            .parse::<i64>()
            .is_ok()
    );

    // Kelas 3: add_user dengan nama dibersihkan dari tab.
    let fourth = get(&app, &uri).await;
    assert_eq!(
        fourth.body,
        format!(
            "C:{add_user}:DATA USER PIN={}\tName=Budi Santoso\tPri=0\n",
            employee.employee_number
        )
    );

    // Antrean habis.
    assert_eq!(get(&app, &uri).await.body, "OK");

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn getrequest_kelas_pengaturan_dikirim_sampai_sepuluh_sekaligus() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());
    for _ in 0..12 {
        // `reboot` bukan perintah pemeliharaan unik, jadi bisa banyak; dipasang di kelas 2
        // hanya untuk menguji batas 10 per poll.
        device.queue(&pool, "reboot", 2).await;
    }

    let reply = get(&app, &format!("/iclock/getrequest?SN={}", device.serial)).await;

    assert_eq!(reply.body.lines().count(), 10);
    assert_eq!(device.count_commands(&pool, "reboot", "pending").await, 2);

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn getrequest_perintah_belum_waktunya_tidak_dikirim() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let id = device.queue(&pool, "check", 1).await;
    sqlx::query(
        "UPDATE device_commands SET available_at = now() + interval '1 hour' WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let app = app(&pool, default_queue());

    let reply = get(&app, &format!("/iclock/getrequest?SN={}", device.serial)).await;

    assert_eq!(reply.body, "OK");
    assert_eq!(device.command_row(&pool, id).await.status, "pending");

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn getrequest_perintah_yang_tidak_bisa_dirakit_ditandai_gagal() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let bad_pin = TestEmployee::create_with_invalid_pin(&pool, "Pin Rusak").await;
    let app = app(&pool, default_queue());
    let uri = format!("/iclock/getrequest?SN={}", device.serial);

    let without_employee = device.queue(&pool, "add_user", 3).await;
    let invalid_pin = device
        .queue_for(&pool, "add_user", 3, Some(bad_pin.id))
        .await;
    let fingerprint = device.queue(&pool, "add_fingerprint", 4).await;

    // Kelas 3 (dua add_user gagal dirakit) lalu kelas 4 (belum didukung): tidak ada baris untuk mesin.
    assert_eq!(get(&app, &uri).await.body, "OK");
    assert_eq!(get(&app, &uri).await.body, "OK");

    for (id, fragment) in [
        (without_employee, "pegawai"),
        (invalid_pin, "PIN"),
        (fingerprint, "belum didukung"),
    ] {
        let row = device.command_row(&pool, id).await;
        assert_eq!(row.status, "failed", "perintah {id}");
        assert!(row.completed);
        assert!(
            row.result_message
                .as_deref()
                .unwrap_or_default()
                .contains(fragment),
            "pesan {:?} tidak memuat {fragment:?}",
            row.result_message
        );
    }

    device.cleanup(&pool).await;
    bad_pin.cleanup(&pool).await;
}

#[tokio::test]
async fn perintah_tanpa_jawaban_dikirim_ulang_lalu_gagal_setelah_batas_percobaan() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let queue = QueueSettings {
        ack_timeout: Duration::from_secs(60),
        max_attempts: 2,
    };
    let app = app(&pool, queue);
    let uri = format!("/iclock/getrequest?SN={}", device.serial);
    let id = device.queue(&pool, "reboot", 1).await;

    // Percobaan 1.
    assert_eq!(get(&app, &uri).await.body, format!("C:{id}:REBOOT\n"));
    // Belum lewat batas waktu: tidak dikirim ulang.
    assert_eq!(get(&app, &uri).await.body, "OK");

    // Lewat batas waktu: kembali pending lalu dikirim lagi (percobaan 2).
    sqlx::query("UPDATE device_commands SET sent_at = now() - interval '5 minutes' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(get(&app, &uri).await.body, format!("C:{id}:REBOOT\n"));
    assert_eq!(device.command_row(&pool, id).await.attempts, 2);

    // Tetap tanpa jawaban: sudah mencapai batas percobaan, ditandai gagal dan tidak dikirim lagi.
    sqlx::query("UPDATE device_commands SET sent_at = now() - interval '5 minutes' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(get(&app, &uri).await.body, "OK");
    let row = device.command_row(&pool, id).await;
    assert_eq!(row.status, "failed");
    assert_eq!(
        row.result_message.as_deref(),
        Some("tidak ada jawaban dari mesin")
    );

    device.cleanup(&pool).await;
}

// ---------------------------------------------------------------- devicecmd

#[tokio::test]
async fn devicecmd_mencatat_sukses_dan_gagal() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());
    let ok = device.queue(&pool, "check", 1).await;
    let bad = device.queue(&pool, "reboot", 1).await;
    for _ in 0..2 {
        get(&app, &format!("/iclock/getrequest?SN={}", device.serial)).await;
    }

    let reply = post(
        &app,
        &format!("/iclock/devicecmd?SN={}", device.serial),
        &format!("ID={ok}&Return=0&CMD=CHECK\nID={bad}&Return=-1001&CMD=REBOOT\n"),
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.content_type, "text/plain; charset=UTF-8");
    assert_eq!(reply.body, "OK\n");
    let ok_row = device.command_row(&pool, ok).await;
    assert_eq!(ok_row.status, "succeeded");
    assert_eq!(ok_row.return_code, Some(0));
    assert!(ok_row.completed);
    let bad_row = device.command_row(&pool, bad).await;
    assert_eq!(bad_row.status, "failed");
    assert_eq!(bad_row.return_code, Some(-1001));
    assert_eq!(
        bad_row.result_message.as_deref(),
        Some("mesin melaporkan Return=-1001")
    );

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn devicecmd_jawaban_kedua_set_time_tidak_menimpa_hasil() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());
    let id = device.queue(&pool, "set_time", 2).await;
    get(&app, &format!("/iclock/getrequest?SN={}", device.serial)).await;

    post(
        &app,
        &format!("/iclock/devicecmd?SN={}", device.serial),
        &format!("ID={id}&Return=0&CMD=SET OPTIONS\nID={id}&Return=-1&CMD=SET OPTIONS\n"),
    )
    .await;

    let row = device.command_row(&pool, id).await;
    assert_eq!(row.status, "succeeded");
    assert_eq!(row.return_code, Some(0));

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn devicecmd_mesin_lain_tidak_bisa_menutup_perintah_milik_mesin_ini() {
    let pool = pool().await;
    let owner = TestDevice::create(&pool).await;
    let other = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());
    let id = owner.queue(&pool, "check", 1).await;
    get(&app, &format!("/iclock/getrequest?SN={}", owner.serial)).await;

    post(
        &app,
        &format!("/iclock/devicecmd?SN={}", other.serial),
        &format!("ID={id}&Return=0&CMD=CHECK\n"),
    )
    .await;

    assert_eq!(owner.command_row(&pool, id).await.status, "sent");

    owner.cleanup(&pool).await;
    other.cleanup(&pool).await;
}

#[tokio::test]
async fn devicecmd_mesin_tak_dikenal_dijawab_ok_tanpa_efek() {
    let pool = pool().await;
    let inactive = TestDevice::create_inactive(&pool).await;
    let id = inactive.queue(&pool, "check", 1).await;
    let app = app(&pool, default_queue());

    for serial in ["TIDAK-TERDAFTAR-3", inactive.serial.as_str()] {
        let reply = post(
            &app,
            &format!("/iclock/devicecmd?SN={serial}"),
            &format!("ID={id}&Return=0&CMD=CHECK\n"),
        )
        .await;
        assert_eq!(reply.body, "OK\n");
    }
    assert_eq!(inactive.command_row(&pool, id).await.status, "pending");
    assert_eq!(inactive.seen(&pool).await, (false, false));

    inactive.cleanup(&pool).await;
}

#[tokio::test]
async fn alur_mesin_baru_info_lalu_check_lalu_bentuk_handshake_berubah() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());
    let handshake = format!(
        "/iclock/cdata?SN={}&options=all&pushver=2.1.44",
        device.serial
    );
    let poll = format!("/iclock/getrequest?SN={}", device.serial);
    let command = format!("/iclock/devicecmd?SN={}", device.serial);

    // 1. Handshake pertama mengantrekan INFO; polling mengirimnya.
    get(&app, &handshake).await;
    let sent = get(&app, &poll).await;
    let info_id: i64 = sent
        .body
        .trim()
        .strip_prefix("C:")
        .and_then(|rest| rest.strip_suffix(":INFO"))
        .and_then(|id| id.parse().ok())
        .expect("seharusnya perintah INFO");

    // 2. Mesin menjawab dengan badan CMD=INFO yang memuat info perangkat.
    let body = format!(
        "ID={info_id}\nReturn=0\nCMD=INFO\nPushVersion=2.1.44\nUserCount=7\nFPCount=9\nFaceCount=0\n\
         TransactionCount=120\n~DeviceName=SpeedFace\n~Platform=ZMM220\nIPAddress=10.0.0.9\n"
    );
    assert_eq!(post(&app, &command, &body).await.body, "OK\n");

    let row: InitializedRow = sqlx::query_as(
        "SELECT initialized_at IS NOT NULL, push_version, user_count, log_count, model, \
                    reported_options ->> 'PushVersion' FROM devices WHERE id = $1",
    )
    .bind(device.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(row.0, "mesin harus terinisialisasi");
    assert_eq!(row.1.as_deref(), Some("2.1.44"));
    assert_eq!((row.2, row.3), (Some(7), Some(120)));
    assert_eq!(row.4.as_deref(), Some("SpeedFace"));
    assert_eq!(row.5.as_deref(), Some("2.1.44"));
    assert_eq!(device.command_row(&pool, info_id).await.status, "succeeded");

    // 3. CHECK menyusul (kode lama) dan dikirim pada polling berikutnya.
    assert_eq!(device.count_commands(&pool, "check", "pending").await, 1);
    assert!(get(&app, &poll).await.body.ends_with(":CHECK\n"));

    // 4. Handshake berikutnya memakai bentuk "push 2.3.0" dan tidak meminta INFO lagi.
    let reply = get(&app, &handshake).await;
    assert!(reply.body.contains("PushProtVer=2.3.0\n"));
    assert_eq!(device.count_commands(&pool, "info", "pending").await, 0);

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn info_gagal_tidak_menandai_mesin_terinisialisasi() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());
    let id = device.queue(&pool, "info", 1).await;
    get(&app, &format!("/iclock/getrequest?SN={}", device.serial)).await;

    post(
        &app,
        &format!("/iclock/devicecmd?SN={}", device.serial),
        &format!("ID={id}\nReturn=-1\nCMD=INFO\n"),
    )
    .await;

    let initialized: bool =
        sqlx::query_scalar("SELECT initialized_at IS NOT NULL FROM devices WHERE id = $1")
            .bind(device.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!initialized);
    assert_eq!(device.command_row(&pool, id).await.status, "failed");

    device.cleanup(&pool).await;
}

#[tokio::test]
async fn getrequest_dengan_user_agent_bukan_mesin_tidak_mengambil_perintah() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let app = app(&pool, default_queue());
    let id = device.queue(&pool, "check", 1).await;

    let reply = send(
        &app,
        Method::GET,
        &format!("/iclock/getrequest?SN={}", device.serial),
        "",
        Some("Mozilla/5.0"),
    )
    .await;

    assert_eq!(reply.body, "OK");
    assert_eq!(device.command_row(&pool, id).await.status, "pending");

    device.cleanup(&pool).await;
}
