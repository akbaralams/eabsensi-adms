//! Uji integrasi Task 7: pemantauan mesin offline dan rotasi kunci biometrik (`rekey`).
//! Butuh `DATABASE_URL` dengan skema v2.

mod common;

use std::sync::Arc;

use common::{TestDevice, TestEmployee, app_with, default_queue, pool, post};
use eabsensi_adms::state::Biometrics;
use eabsensi_adms::vault::{Aad, Vault};
use eabsensi_adms::{offline, rekey};
use sqlx::PgPool;
use zeroize::Zeroizing;

const KEY_A: [u8; 32] = [11; 32];
const KEY_B: [u8; 32] = [22; 32];
const KEY_C: [u8; 32] = [33; 32];

// ------------------------------------------------------------ mesin offline

#[tokio::test]
async fn check_offline_memakai_batas_per_mesin_dan_mengabaikan_yang_nonaktif() {
    let pool = pool().await;
    let online = TestDevice::create(&pool).await;
    online
        .set(&pool, "last_seen_at = now() - interval '10 seconds'")
        .await;
    let stale = TestDevice::create(&pool).await;
    stale
        .set(&pool, "last_seen_at = now() - interval '10 minutes'")
        .await;
    let never = TestDevice::create(&pool).await;
    // Batas = max(3 x 60, 120) = 180 detik; 100 detik masih online.
    let borderline = TestDevice::create(&pool).await;
    borderline
        .set(&pool, "last_seen_at = now() - interval '100 seconds'")
        .await;
    // Polling jarang (600 detik): batas 1800 detik, 20 menit masih online.
    let slow = TestDevice::create(&pool).await;
    slow.set(
        &pool,
        "poll_delay_seconds = 600, last_seen_at = now() - interval '20 minutes'",
    )
    .await;
    // Mesin yang dimatikan operator tidak dihitung walau lama tidak terlihat.
    let retired = TestDevice::create_inactive(&pool).await;
    retired
        .set(&pool, "last_seen_at = now() - interval '30 days'")
        .await;

    let ours = [&online, &stale, &never, &borderline, &slow, &retired];
    let found = offline::find(&pool, 3, 120).await.unwrap();
    let offline_serials: Vec<&str> = found
        .iter()
        .map(|device| device.serial.as_str())
        .filter(|serial| ours.iter().any(|d| d.serial == *serial))
        .collect();

    // Urutan: yang belum pernah terhubung dulu, lalu yang paling lama tidak terlihat.
    assert_eq!(
        offline_serials,
        vec![never.serial.as_str(), stale.serial.as_str()]
    );

    let stale_row = found.iter().find(|d| d.serial == stale.serial).unwrap();
    assert!((stale_row.seconds_since_seen.unwrap() - 600.0).abs() < 5.0);
    assert!((stale_row.threshold_seconds - 180.0).abs() < f64::EPSILON);
    assert!(
        found
            .iter()
            .find(|d| d.serial == never.serial)
            .unwrap()
            .seconds_since_seen
            .is_none()
    );

    // Faktor dan batas minimum dapat diubah: batas 1 x 60 dengan minimum 30 detik.
    let strict = offline::find(&pool, 1, 30).await.unwrap();
    assert!(
        strict.iter().any(|d| d.serial == borderline.serial),
        "100 detik > 60 detik"
    );

    let text = offline::render(&found);
    assert!(text.contains(&format!("OFFLINE {} ", stale.serial)));
    assert!(text.contains("belum pernah terhubung"));

    for device in [online, stale, never, borderline, slow, retired] {
        device.cleanup(&pool).await;
    }
}

// ------------------------------------------------------------------- rekey

fn vault(id: &str, key: &[u8; 32], old: &[(&str, [u8; 32])]) -> Arc<Vault> {
    let old: Vec<(String, Zeroizing<[u8; 32]>)> = old
        .iter()
        .map(|(id, key)| ((*id).to_owned(), Zeroizing::new(*key)))
        .collect();
    Arc::new(Vault::new((id, key), &old).unwrap())
}

/// id kunci unik per uji (maks 16 karakter) supaya uji paralel tidak saling menyentuh baris.
fn key_id(tag: &str) -> String {
    format!("{tag}{}", std::process::id() % 100_000)
}

async fn upload(
    pool: &PgPool,
    v: Arc<Vault>,
    device: &TestDevice,
    employee: &TestEmployee,
    finger: i16,
    tmp: &str,
) {
    let app = app_with(
        pool,
        default_queue(),
        Biometrics {
            vault: Some(v),
            store_face_photo: false,
        },
    );
    let body = format!(
        "FP PIN={}\tFID={finger}\tSize=6\tValid=1\tTMP={tmp}\n",
        employee.employee_number
    );
    let reply = post(
        &app,
        &format!("/iclock/cdata?SN={}&table=OPERLOG&OpStamp=1", device.serial),
        &body,
    )
    .await;
    assert!(reply.body.starts_with("OK"));
}

async fn rows(pool: &PgPool, employee: &TestEmployee) -> Vec<(i64, String, i16, Vec<u8>, Vec<u8>)> {
    sqlx::query_as(
        "SELECT id, key_id, finger_index, ciphertext, nonce \
           FROM biometric_templates WHERE employee_id = $1 ORDER BY finger_index",
    )
    .bind(employee.id)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn open(v: &Vault, employee: &TestEmployee, row: &(i64, String, i16, Vec<u8>, Vec<u8>)) -> Vec<u8> {
    let aad = Aad {
        employee_id: employee.id,
        kind: "fingerprint",
        finger_index: row.2,
        algorithm_version: "",
    };
    v.decrypt(&aad, &row.3, &row.4, &row.1).unwrap()
}

#[tokio::test]
async fn rekey_memindahkan_semua_baris_ke_kunci_terkini_dan_aman_diulang() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let one = TestEmployee::create(&pool, "Satu").await;
    let two = TestEmployee::create(&pool, "Dua").await;
    let other = TestEmployee::create(&pool, "Kunci Lain").await;
    let (a, b, c) = (key_id("a"), key_id("b"), key_id("c"));

    upload(&pool, vault(&a, &KEY_A, &[]), &device, &one, 1, "AQIDBAUG").await;
    upload(&pool, vault(&a, &KEY_A, &[]), &device, &one, 2, "BwgJCgsM").await;
    upload(&pool, vault(&a, &KEY_A, &[]), &device, &two, 1, "AQIDBAUG").await;
    // Baris di bawah kunci lain yang tidak dikenal vault tujuan; harus tak tersentuh oleh `from`.
    upload(
        &pool,
        vault(&c, &KEY_C, &[]),
        &device,
        &other,
        1,
        "AQIDBAUG",
    )
    .await;
    let other_before = rows(&pool, &other).await;

    let rotated = vault(&b, &KEY_B, &[(a.as_str(), KEY_A)]);

    // Dry-run: menghitung dan memastikan semua bisa dibuka, tanpa mengubah apa pun.
    let dry = rekey::run(&pool, &rotated, Some(&a), true).await.unwrap();
    assert_eq!(
        (dry.would_reseal, dry.resealed, dry.failed.len()),
        (3, 0, 0)
    );
    assert!(rows(&pool, &one).await.iter().all(|row| row.1 == a));

    // Sungguhan: tiga baris pindah ke kunci b; plaintext utuh dan hanya kunci b yang dibutuhkan.
    let before = rows(&pool, &one).await;
    let real = rekey::run(&pool, &rotated, Some(&a), false).await.unwrap();
    assert_eq!(
        (real.resealed, real.failed.len(), real.overwritten),
        (3, 0, 0)
    );

    let only_b = vault(&b, &KEY_B, &[]);
    let after = rows(&pool, &one).await;
    assert!(after.iter().all(|row| row.1 == b));
    for (old, new) in before.iter().zip(&after) {
        assert_eq!(
            open(&only_b, &one, new),
            open(&vault(&a, &KEY_A, &[]), &one, old)
        );
        assert_ne!(old.3, new.3, "ciphertext baru");
        assert_ne!(old.4, new.4, "nonce baru");
    }
    assert!(rows(&pool, &two).await.iter().all(|row| row.1 == b));

    // Baris di bawah kunci lain tidak disentuh, dan tidak dihitung gagal berkat `from`.
    assert_eq!(rows(&pool, &other).await, other_before);

    // Diulang: tidak ada lagi yang perlu dipindah.
    let again = rekey::run(&pool, &rotated, Some(&a), false).await.unwrap();
    assert_eq!(
        (again.resealed, again.would_reseal, again.failed.len()),
        (0, 0, 0)
    );

    device.cleanup(&pool).await;
    for employee in [one, two, other] {
        employee.cleanup(&pool).await;
    }
}

#[tokio::test]
async fn rekey_melaporkan_baris_yang_tidak_bisa_dibuka_tanpa_mengubahnya() {
    let pool = pool().await;
    let device = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Satu").await;
    let (a, b) = (key_id("x"), key_id("y"));

    upload(
        &pool,
        vault(&a, &KEY_A, &[]),
        &device,
        &employee,
        1,
        "AQIDBAUG",
    )
    .await;
    upload(
        &pool,
        vault(&a, &KEY_A, &[]),
        &device,
        &employee,
        2,
        "BwgJCgsM",
    )
    .await;
    let before = rows(&pool, &employee).await;
    // Merusak baris pertama: ciphertext tidak lagi lolos autentikasi.
    sqlx::query("UPDATE biometric_templates SET ciphertext = '\\x0001020304'::bytea WHERE id = $1")
        .bind(before[0].0)
        .execute(&pool)
        .await
        .unwrap();

    // Vault tujuan tidak memuat kunci lama sama sekali: kedua baris tidak bisa dibuka.
    let missing_old = vault(&b, &KEY_B, &[]);
    let report = rekey::run(&pool, &missing_old, Some(&a), false)
        .await
        .unwrap();
    assert_eq!((report.resealed, report.failed.len()), (0, 2));
    assert!(
        rows(&pool, &employee).await.iter().all(|row| row.1 == a),
        "tidak ada yang berubah"
    );

    // Dengan kunci lama: satu baris sehat berpindah, satu baris rusak dilaporkan dan tak diubah.
    let rotated = vault(&b, &KEY_B, &[(a.as_str(), KEY_A)]);
    let report = rekey::run(&pool, &rotated, Some(&a), false).await.unwrap();
    assert_eq!(
        (report.resealed, report.failed.clone()),
        (1, vec![before[0].0])
    );
    let after = rows(&pool, &employee).await;
    assert_eq!(after[0].1, a, "baris rusak tetap apa adanya");
    assert_eq!(after[1].1, b);

    let text = rekey::render(&report, false, &b);
    assert!(text.contains("GAGAL: 1 baris"));
    assert!(text.contains(&before[0].0.to_string()));

    device.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}
