//! Uji integrasi Task 5b: template biometrik terenkripsi (unggah, kirim ulang, rotasi kunci, kirim
//! ke mesin lain) dan jaminan tidak ada isi template di log. Butuh `DATABASE_URL` dengan skema v2.

mod common;

use std::sync::Arc;

use common::{TestDevice, TestEmployee, app, app_with, default_queue, get, pool, post};
use eabsensi_adms::state::Biometrics;
use eabsensi_adms::vault::{Aad, Vault};
use sqlx::PgPool;
use zeroize::Zeroizing;

const KEY_1: [u8; 32] = [7; 32];
const KEY_2: [u8; 32] = [9; 32];
/// base64 dari bytes 1..=6.
const TMP_A: &str = "AQIDBAUG";
/// base64 dari bytes 7..=12.
const TMP_B: &str = "BwgJCgsM";

/// kind, nomor jari, versi algoritma, valid, ukuran asli, key_id, ciphertext, nonce, mesin asal.
type TemplateRow = (
    String,
    i16,
    String,
    bool,
    i32,
    String,
    Vec<u8>,
    Vec<u8>,
    Option<i64>,
);

fn vault(id: &str, key: &[u8; 32], old: &[(&str, [u8; 32])]) -> Arc<Vault> {
    let old: Vec<(String, Zeroizing<[u8; 32]>)> = old
        .iter()
        .map(|(id, key)| ((*id).to_owned(), Zeroizing::new(*key)))
        .collect();
    Arc::new(Vault::new((id, key), &old).unwrap())
}

fn biometrics(vault: Arc<Vault>, photos: bool) -> Biometrics {
    Biometrics {
        vault: Some(vault),
        store_face_photo: photos,
    }
}

fn operlog_uri(device: &TestDevice, stamp: u64) -> String {
    format!(
        "/iclock/cdata?SN={}&table=OPERLOG&OpStamp={stamp}",
        device.serial
    )
}

fn fp_row(pin: &str, fid: i16, tmp: &str) -> String {
    format!("FP PIN={pin}\tFID={fid}\tSize=6\tValid=1\tTMP={tmp}")
}

async fn templates_of(pool: &PgPool, employee: &TestEmployee) -> Vec<TemplateRow> {
    sqlx::query_as(
        "SELECT kind, finger_index, algorithm_version, is_valid, original_size, key_id, \
                ciphertext, nonce, source_device_id \
           FROM biometric_templates WHERE employee_id = $1 ORDER BY kind, finger_index, algorithm_version",
    )
    .bind(employee.id)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn decrypt(vault: &Vault, employee: &TestEmployee, row: &TemplateRow) -> Vec<u8> {
    let aad = Aad {
        employee_id: employee.id,
        kind: &row.0,
        finger_index: row.1,
        algorithm_version: &row.2,
    };
    vault.decrypt(&aad, &row.6, &row.7, &row.5).unwrap()
}

async fn template_id(pool: &PgPool, employee: &TestEmployee) -> i64 {
    sqlx::query_scalar(
        "SELECT id FROM biometric_templates WHERE employee_id = $1 ORDER BY id LIMIT 1",
    )
    .bind(employee.id)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ------------------------------------------------------------------ unggah

#[tokio::test]
async fn template_sidik_jari_disimpan_terenkripsi_dan_oplog_diabaikan() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let v = vault("k1", &KEY_1, &[]);
    let app = app_with(&pool, default_queue(), biometrics(v.clone(), false));

    let body = format!(
        "OPLOG 4\t0\t2024-06-05 10:27:20\t0\t0\t0\t0\nUSER PIN={pin}\tName=Budi\tPri=0\n{}\n",
        fp_row(pin, 2, TMP_A)
    );
    let reply = post(&app, &operlog_uri(&device, 785154440), &body).await;

    assert_eq!(reply.body, "OK:3\n");
    let rows = templates_of(&pool, &employee).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        (row.0.as_str(), row.1, row.2.as_str(), row.3, row.4),
        ("fingerprint", 2, "", true, 6)
    );
    assert_eq!(row.5, "k1");
    assert_eq!(row.8, Some(device.id));
    assert_ne!(
        row.6,
        vec![1, 2, 3, 4, 5, 6],
        "isi tidak boleh tersimpan polos"
    );
    assert_eq!(row.6.len(), 6 + 16, "ciphertext + tag GCM");
    assert_eq!(row.7.len(), 12);
    assert_eq!(decrypt(&v, &employee, row), vec![1, 2, 3, 4, 5, 6]);

    // Stamp OPERLOG disimpan di transaksi yang sama.
    let stamp: Option<String> =
        sqlx::query_scalar("SELECT operlog_stamp FROM devices WHERE id = $1")
            .bind(device.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stamp.as_deref(), Some("785154440"));

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn kiriman_ulang_tidak_menulis_ulang_dan_perubahan_menimpa_barisnya() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let v = vault("k1", &KEY_1, &[]);
    let app = app_with(&pool, default_queue(), biometrics(v.clone(), false));
    let uri = operlog_uri(&device, 1);

    post(&app, &uri, &format!("{}\n", fp_row(pin, 2, TMP_A))).await;
    let first = templates_of(&pool, &employee).await;

    for _ in 0..3 {
        post(&app, &uri, &format!("{}\n", fp_row(pin, 2, TMP_A))).await;
    }
    let after_resend = templates_of(&pool, &employee).await;
    assert_eq!(after_resend.len(), 1);
    assert_eq!(
        after_resend[0].6, first[0].6,
        "ciphertext sama: tidak ditulis ulang"
    );
    assert_eq!(
        after_resend[0].7, first[0].7,
        "nonce sama: tidak ditulis ulang"
    );

    // Jari yang sama dengan isi baru menimpa baris yang ada (satu baris per jari).
    post(&app, &uri, &format!("{}\n", fp_row(pin, 2, TMP_B))).await;
    let changed = templates_of(&pool, &employee).await;
    assert_eq!(changed.len(), 1);
    assert_eq!(
        decrypt(&v, &employee, &changed[0]),
        vec![7, 8, 9, 10, 11, 12]
    );

    // Jari lain menambah baris.
    post(&app, &uri, &format!("{}\n", fp_row(pin, 5, TMP_A))).await;
    assert_eq!(templates_of(&pool, &employee).await.len(), 2);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn pin_tak_dikenal_dibuang_dan_versi_algoritma_dari_mesin_dipakai() {
    let pool = pool().await;
    let device_10 = TestDevice::create(&pool).await;
    device_10
        .set(
            &pool,
            "reported_options = '{\"~ZKFPVersion\": \"10\"}'::jsonb",
        )
        .await;
    let device_12 = TestDevice::create(&pool).await;
    device_12
        .set(
            &pool,
            "reported_options = '{\"~ZKFPVersion\": \"12\"}'::jsonb",
        )
        .await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app_with(
        &pool,
        default_queue(),
        biometrics(vault("k1", &KEY_1, &[]), false),
    );

    let body = format!(
        "{}\n{}\n",
        fp_row(pin, 0, TMP_A),
        fp_row("TIDAKADA", 0, TMP_A)
    );
    post(&app, &operlog_uri(&device_10, 1), &body).await;
    post(
        &app,
        &operlog_uri(&device_12, 1),
        &format!("{}\n", fp_row(pin, 0, TMP_B)),
    )
    .await;

    let rows = templates_of(&pool, &employee).await;
    assert_eq!(rows.len(), 2, "versi 10 dan 12 tidak saling menimpa");
    assert_eq!(rows[0].2, "10");
    assert_eq!(rows[1].2, "12");
    let unknown: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM biometric_templates WHERE employee_id NOT IN (SELECT id FROM employees)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unknown, 0);

    device_10.cleanup(&pool).await;
    device_12.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn tabledata_templatev10_dan_foto_wajah_menurut_pengaturan() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let sn = &device.serial;
    let templatev10 =
        format!("templatev10 pin={pin}\tfingerid=3\tsize=6\tvalid=1\ttemplate={TMP_A}\n");
    let biophoto = format!("biophoto pin={pin}\tfilename=1.jpg\ttype=9\tsize=6\tcontent={TMP_B}\n");

    // Foto wajah mati (bawaan): hanya template jari yang masuk.
    let v = vault("k1", &KEY_1, &[]);
    let off = app_with(&pool, default_queue(), biometrics(v.clone(), false));
    let reply = post(
        &off,
        &format!("/iclock/cdata?SN={sn}&table=tabledata&tablename=templatev10&count=1"),
        &templatev10,
    )
    .await;
    assert_eq!(reply.body, "OK\n");
    post(
        &off,
        &format!("/iclock/cdata?SN={sn}&table=tabledata&tablename=biophoto&count=1"),
        &biophoto,
    )
    .await;
    let rows = templates_of(&pool, &employee).await;
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].0.as_str(), rows[0].1), ("fingerprint", 3));

    // Foto wajah dinyalakan: foto ikut disimpan terenkripsi.
    let on = app_with(&pool, default_queue(), biometrics(v.clone(), true));
    post(
        &on,
        &format!("/iclock/cdata?SN={sn}&table=tabledata&tablename=biophoto&count=1"),
        &biophoto,
    )
    .await;
    let rows = templates_of(&pool, &employee).await;
    assert_eq!(rows.len(), 2);
    let photo = rows
        .iter()
        .find(|row| row.0 == "face_photo")
        .expect("foto wajah");
    assert_eq!(photo.1, 0);
    assert_eq!(decrypt(&v, &employee, photo), vec![7, 8, 9, 10, 11, 12]);

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn tanpa_kunci_template_dibuang_tetapi_stamp_dan_balasan_tetap() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app(&pool, default_queue()); // brankas mati

    let reply = post(
        &app,
        &operlog_uri(&device, 4242),
        &format!("{}\n", fp_row(pin, 0, TMP_A)),
    )
    .await;

    assert_eq!(reply.body, "OK:1\n");
    assert!(
        templates_of(&pool, &employee).await.is_empty(),
        "tidak pernah disimpan tanpa enkripsi"
    );
    let stamp: Option<String> =
        sqlx::query_scalar("SELECT operlog_stamp FROM devices WHERE id = $1")
            .bind(device.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stamp.as_deref(), Some("4242"));

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn rotasi_kunci_baris_lama_dienkripsi_ulang_ke_kunci_terkini() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let uri = operlog_uri(&device, 1);
    let body = format!("{}\n", fp_row(pin, 1, TMP_A));

    let old = app_with(
        &pool,
        default_queue(),
        biometrics(vault("k1", &KEY_1, &[]), false),
    );
    post(&old, &uri, &body).await;
    assert_eq!(templates_of(&pool, &employee).await[0].5, "k1");

    // Kunci terkini k2; k1 hanya untuk membaca. Kiriman ulang yang sama memigrasikan baris.
    let rotated = vault("k2", &KEY_2, &[("k1", KEY_1)]);
    let new = app_with(&pool, default_queue(), biometrics(rotated.clone(), false));
    post(&new, &uri, &body).await;

    let rows = templates_of(&pool, &employee).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].5, "k2");
    assert_eq!(
        decrypt(&rotated, &employee, &rows[0]),
        vec![1, 2, 3, 4, 5, 6]
    );

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

// -------------------------------------------------------- kirim ke mesin lain

async fn upload_template(
    pool: &PgPool,
    v: Arc<Vault>,
    source: &TestDevice,
    employee: &TestEmployee,
) {
    let app = app_with(pool, default_queue(), biometrics(v, false));
    post(
        &app,
        &operlog_uri(source, 1),
        &format!("{}\n", fp_row(&employee.employee_number, 2, TMP_A)),
    )
    .await;
}

#[tokio::test]
async fn add_fingerprint_mengirim_template_dari_mesin_lain_dan_hasilnya_dicatat() {
    let pool = pool().await;
    let source = TestDevice::create(&pool).await;
    let target = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let v = vault("k1", &KEY_1, &[]);
    upload_template(&pool, v.clone(), &source, &employee).await;
    let template = template_id(&pool, &employee).await;
    let command = sqlx::query_scalar::<_, i64>(
        "INSERT INTO device_commands (device_id, command_type, priority, status, employee_id, \
                biometric_template_id, created_at, updated_at) \
         VALUES ($1, 'add_fingerprint', 4, 'pending', $2, $3, now(), now()) RETURNING id",
    )
    .bind(target.id)
    .bind(employee.id)
    .bind(template)
    .fetch_one(&pool)
    .await
    .unwrap();

    let app = app_with(&pool, default_queue(), biometrics(v, false));
    let reply = get(&app, &format!("/iclock/getrequest?SN={}", target.serial)).await;

    assert_eq!(
        reply.body,
        format!(
            "C:{command}:DATA FP PIN={}\tFID=2\tSize=6\tValid=1\tTMP={TMP_A}\n",
            employee.employee_number
        )
    );
    assert_eq!(target.command_row(&pool, command).await.status, "sent");

    post(
        &app,
        &format!("/iclock/devicecmd?SN={}", target.serial),
        &format!("ID={command}&Return=0&CMD=DATA\n"),
    )
    .await;
    assert_eq!(target.command_row(&pool, command).await.status, "succeeded");

    source.cleanup(&pool).await;
    target.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}

#[tokio::test]
async fn add_fingerprint_yang_tidak_boleh_dikirim_ditandai_gagal_tanpa_membocorkan_isi() {
    let pool = pool().await;
    let source = TestDevice::create(&pool).await;
    let target = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let other = TestEmployee::create(&pool, "Lain").await;
    let v = vault("k1", &KEY_1, &[]);
    upload_template(&pool, v.clone(), &source, &employee).await;
    let template = template_id(&pool, &employee).await;

    let queue = |employee_id: Option<i64>, template_id: Option<i64>| {
        let pool = pool.clone();
        let device = target.id;
        async move {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO device_commands (device_id, command_type, priority, status, employee_id, \
                        biometric_template_id, created_at, updated_at) \
                 VALUES ($1, 'add_fingerprint', 4, 'pending', $2, $3, now(), now()) RETURNING id",
            )
            .bind(device)
            .bind(employee_id)
            .bind(template_id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let uri = format!("/iclock/getrequest?SN={}", target.serial);
    let app = app_with(&pool, default_queue(), biometrics(v.clone(), false));

    // Tanpa rujukan template.
    let no_ref = queue(Some(employee.id), None).await;
    // Template milik pegawai lain.
    let wrong_owner = queue(Some(other.id), Some(template)).await;
    // Template ditandai tidak valid.
    let invalid = queue(Some(employee.id), Some(template)).await;
    sqlx::query("UPDATE biometric_templates SET is_valid = false WHERE id = $1")
        .bind(template)
        .execute(&pool)
        .await
        .unwrap();
    for _ in 0..3 {
        assert_eq!(get(&app, &uri).await.body, "OK");
    }
    for (id, fragment) in [
        (no_ref, "tidak merujuk template"),
        (wrong_owner, "bukan milik"),
        (invalid, "tidak valid"),
    ] {
        let row = target.command_row(&pool, id).await;
        assert_eq!(row.status, "failed", "perintah {id}");
        assert!(
            row.result_message
                .as_deref()
                .unwrap_or_default()
                .contains(fragment),
            "{:?}",
            row.result_message
        );
    }

    // Versi algoritma mesin tujuan berbeda.
    sqlx::query(
        "UPDATE biometric_templates SET is_valid = true, algorithm_version = '10' WHERE id = $1",
    )
    .bind(template)
    .execute(&pool)
    .await
    .unwrap();
    target
        .set(
            &pool,
            "reported_options = '{\"~ZKFPVersion\": \"12\"}'::jsonb",
        )
        .await;
    let mismatch = queue(Some(employee.id), Some(template)).await;
    assert_eq!(get(&app, &uri).await.body, "OK");
    let row = target.command_row(&pool, mismatch).await;
    assert_eq!(row.status, "failed");
    assert!(row.result_message.unwrap().contains("versi algoritma"));

    // Baris yang tidak bisa dibuka: versi di baris ('10') berbeda dari versi saat dienkripsi ('')
    // sehingga AAD tidak cocok, sama seperti kunci yang salah. Mesin tujuan tidak melaporkan
    // versi, jadi pemeriksaan versi dilewati dan yang gagal adalah dekripsinya.
    target.set(&pool, "reported_options = NULL").await;
    let undecryptable = queue(Some(employee.id), Some(template)).await;
    assert_eq!(get(&app, &uri).await.body, "OK");
    let row = target.command_row(&pool, undecryptable).await;
    assert_eq!(row.status, "failed");
    let message = row.result_message.unwrap();
    assert!(message.contains("tidak dapat dibuka"), "{message}");
    assert!(!message.contains(TMP_A) && !message.contains("k1"));

    // Brankas dimatikan: perintah gagal dengan alasan yang jelas.
    let plain_app = app_with(&pool, default_queue(), Biometrics::disabled());
    let no_vault = queue(Some(employee.id), Some(template)).await;
    assert_eq!(get(&plain_app, &uri).await.body, "OK");
    assert!(
        target
            .command_row(&pool, no_vault)
            .await
            .result_message
            .unwrap()
            .contains("kunci biometrik")
    );

    source.cleanup(&pool).await;
    target.cleanup(&pool).await;
    employee.cleanup(&pool).await;
    other.cleanup(&pool).await;
}

#[tokio::test]
async fn kunci_lama_tetap_dipakai_untuk_mengirim_setelah_rotasi() {
    let pool = pool().await;
    let source = TestDevice::create(&pool).await;
    let target = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    upload_template(&pool, vault("k1", &KEY_1, &[]), &source, &employee).await;
    let template = template_id(&pool, &employee).await;
    let command = sqlx::query_scalar::<_, i64>(
        "INSERT INTO device_commands (device_id, command_type, priority, status, employee_id, \
                biometric_template_id, created_at, updated_at) \
         VALUES ($1, 'add_fingerprint', 4, 'pending', $2, $3, now(), now()) RETURNING id",
    )
    .bind(target.id)
    .bind(employee.id)
    .bind(template)
    .fetch_one(&pool)
    .await
    .unwrap();

    let rotated = app_with(
        &pool,
        default_queue(),
        biometrics(vault("k2", &KEY_2, &[("k1", KEY_1)]), false),
    );
    let reply = get(
        &rotated,
        &format!("/iclock/getrequest?SN={}", target.serial),
    )
    .await;

    assert!(
        reply.body.contains(&format!("C:{command}:DATA FP PIN=")),
        "{}",
        reply.body
    );
    assert!(reply.body.contains(&format!("TMP={TMP_A}")));

    source.cleanup(&pool).await;
    target.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}
