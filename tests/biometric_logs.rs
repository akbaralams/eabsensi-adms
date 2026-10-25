//! Isi template biometrik tidak boleh muncul di log (keputusan D1).
//!
//! Sengaja satu uji di berkas sendiri (proses sendiri): penangkap log memakai subscriber
//! thread-local, dan uji lain yang berjalan paralel di proses yang sama bisa mendaftarkan titik
//! log `tracing` lebih dulu sehingga log uji ini tidak tertangkap.

mod common;

use std::io;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::{TestDevice, TestEmployee, app_with, default_queue, get, pool, post};
use eabsensi_adms::state::Biometrics;
use eabsensi_adms::vault::Vault;
use zeroize::Zeroizing;

const KEY_1: [u8; 32] = [7; 32];
const KEY_2: [u8; 32] = [9; 32];

fn vault(key: &[u8; 32]) -> Arc<Vault> {
    Arc::new(Vault::new(("k1", key), &[] as &[(String, Zeroizing<[u8; 32]>)]).unwrap())
}

fn biometrics(key: &[u8; 32]) -> Biometrics {
    Biometrics {
        vault: Some(vault(key)),
        store_face_photo: true,
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn isi_template_tidak_pernah_muncul_di_log() {
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    // Runtime `current_thread`: seluruh pekerjaan uji ini berjalan di satu thread.
    let _guard = tracing::subscriber::set_default(subscriber);

    let pool = pool().await;
    let source = TestDevice::create(&pool).await;
    let target = TestDevice::create(&pool).await;
    let employee = TestEmployee::create(&pool, "Budi").await;
    let pin = &employee.employee_number;
    let app = app_with(&pool, default_queue(), biometrics(&KEY_1));

    // Isi yang dikenali: tidak boleh muncul, baik sebagai base64 maupun sebagai teks.
    let secret = b"TEMPLATE-RAHASIA-JANGAN-DICATAT-0123456789";
    let secret_b64 = STANDARD.encode(secret);
    let body = format!(
        "FP PIN={pin}\tFID=1\tSize=42\tValid=1\tTMP={secret_b64}\n\
         FP PIN=TIDAKADA\tFID=1\tSize=42\tValid=1\tTMP={secret_b64}\n\
         FP PIN={pin}\tFID=2\tSize=42\tValid=1\tTMP=bukan-base64!!\n\
         BIOPHOTO PIN={pin}\tFileName=a.jpg\tType=9\tSize=42\tContent={secret_b64}\n"
    );
    post(
        &app,
        &format!("/iclock/cdata?SN={}&table=OPERLOG&OpStamp=1", source.serial),
        &body,
    )
    .await;
    post(
        &app,
        &format!(
            "/iclock/cdata?SN={}&table=tabledata&tablename=templatev10&count=1",
            source.serial
        ),
        &format!("templatev10 pin={pin}\tfingerid=4\tsize=42\tvalid=1\ttemplate={secret_b64}\n"),
    )
    .await;

    // Kirim ke mesin lain, termasuk jalur gagal (kunci server salah).
    let template: i64 = sqlx::query_scalar(
        "SELECT id FROM biometric_templates WHERE employee_id = $1 AND kind = 'fingerprint' ORDER BY id LIMIT 1",
    )
    .bind(employee.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let queue_send = || async {
        sqlx::query(
            "INSERT INTO device_commands (device_id, command_type, priority, status, employee_id,                     biometric_template_id, created_at, updated_at)              VALUES ($1, 'add_fingerprint', 4, 'pending', $2, $3, now(), now())",
        )
        .bind(target.id)
        .bind(employee.id)
        .bind(template)
        .execute(&pool)
        .await
        .unwrap();
    };
    queue_send().await;
    let sent = get(&app, &format!("/iclock/getrequest?SN={}", target.serial)).await;
    assert!(
        sent.body.contains(&secret_b64),
        "uji ini harus benar-benar mengirim template"
    );
    queue_send().await;
    let wrong_key = app_with(&pool, default_queue(), biometrics(&KEY_2));
    get(
        &wrong_key,
        &format!("/iclock/getrequest?SN={}", target.serial),
    )
    .await;

    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("template biometrik diproses"),
        "log penerimaan harus ada agar uji ini bermakna: {logs}"
    );
    assert!(logs.contains("template tidak dapat didekripsi"), "{logs}");
    assert!(
        !logs.contains(&secret_b64),
        "base64 template tidak boleh ada di log"
    );
    assert!(
        !logs.contains("TEMPLATE-RAHASIA"),
        "isi template tidak boleh ada di log"
    );
    assert!(
        !logs.contains("bukan-base64"),
        "isi baris tidak boleh ada di log"
    );
    assert!(
        !logs.contains(&STANDARD.encode(KEY_1)) && !logs.contains(&STANDARD.encode(KEY_2)),
        "kunci tidak boleh ada di log"
    );

    source.cleanup(&pool).await;
    target.cleanup(&pool).await;
    employee.cleanup(&pool).await;
}
