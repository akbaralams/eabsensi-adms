//! Endpoint unggah data dan pendamping: `POST /iclock/cdata`, `fdata`, `registry`, `push`,
//! `rtdata`, `ping`, dan `querydata`.
//!
//! Keputusan produk (D2, D6, D7, A1) ada di `docs/schema-proposal.md`; rinciannya di
//! `docs/implementation-notes.md`. Mesin tak terdaftar atau nonaktif tidak mengubah apa pun dan
//! dijawab kosong, **bukan** `OK`: jawaban `OK` membuat mesin menganggap data terkirim dan
//! menghapusnya, padahal mesin itu belum terdaftar.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Extension, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use chrono::Utc;
use rand::distr::{Alphanumeric, SampleString};
use serde::Deserialize;

use super::iclock::{AppError, empty, plain, serial_of, zone_of};
use crate::db::commands;
use crate::db::devices::{self, Stamp};
use crate::ingest;
use crate::ingest::biometrics as bio_ingest;
use crate::protocol::attlog::{self, Upload};
use crate::protocol::biometric;
use crate::protocol::command::CommandKind;
use crate::protocol::info::{self, DeviceInfo};
use crate::protocol::kv;
use crate::protocol::session;
use crate::protocol::zk_time;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    #[serde(rename = "SN")]
    sn: Option<String>,
    table: Option<String>,
    #[serde(rename = "Stamp")]
    stamp: Option<String>,
    #[serde(rename = "OpStamp")]
    op_stamp: Option<String>,
    #[serde(rename = "PhotoStamp")]
    photo_stamp: Option<String>,
    tablename: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SnQuery {
    #[serde(rename = "SN")]
    sn: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RtDataQuery {
    #[serde(rename = "SN")]
    sn: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueryData {
    #[serde(rename = "SN")]
    sn: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    tablename: Option<String>,
    count: Option<String>,
}

fn non_empty_lines(body: &str) -> usize {
    body.lines().filter(|line| !line.trim().is_empty()).count()
}

/// Stamp dari parameter query bila berbentuk angka wajar.
fn stamp_of(value: Option<&String>) -> Option<&str> {
    value.and_then(|v| attlog::valid_stamp(v))
}

fn client_ip(connection: Option<&Extension<ConnectInfo<SocketAddr>>>) -> Option<String> {
    connection.map(|Extension(ConnectInfo(addr))| addr.ip().to_string())
}

/// `POST /iclock/cdata`: unggahan data mesin. Dipilah menurut `table` dengan urutan `idata` lama.
pub async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UploadQuery>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    body: Bytes,
) -> Result<Response, AppError> {
    // Bukan request mesin: kode lama jatuh ke balasan akhir `OK`.
    let Some(serial) = serial_of(&headers, query.sn.as_ref()) else {
        return Ok(plain(session::render_upload_ok(0)));
    };
    let Some(device) = devices::find_active(&state.db, &serial).await? else {
        tracing::warn!(serial = %serial, "unggahan dari mesin tidak terdaftar atau nonaktif ditolak");
        return Ok(empty());
    };
    devices::touch(&state.db, device.id).await?;

    let kind = attlog::classify(
        query.table.as_deref(),
        query.op_stamp.is_some(),
        query.stamp.is_some(),
        query.photo_stamp.is_some(),
    );
    let text = String::from_utf8_lossy(&body);

    match kind {
        Upload::RtState | Upload::RtLog => Ok(plain("OK")),
        Upload::Options => {
            let pairs = kv::pairs(&text, true);
            devices::update_info(
                &state.db,
                device.id,
                &DeviceInfo::from_pairs(&pairs),
                Some(&info::bounded_options(&pairs)),
            )
            .await?;
            Ok(plain("OK"))
        }
        Upload::TableData => {
            if matches!(query.tablename.as_deref(), Some("templatev10" | "biophoto")) {
                let parsed = biometric::parse_tabledata(
                    query.tablename.as_deref().unwrap_or_default(),
                    &text,
                );
                receive_biometrics(&state, &device, &serial, parsed, None).await?;
            }
            Ok(plain(session::render_upload_ok(0)))
        }
        Upload::Unknown => Ok(plain(session::render_upload_ok(0))),
        Upload::Operation => {
            // OPLOG dan USER diabaikan (D6); baris FP/FACE/BIOPHOTO disimpan terenkripsi (D1).
            let stamp = stamp_of(query.op_stamp.as_ref()).map(|value| (Stamp::OperLog, value));
            let parsed = if state.biometrics.vault.is_some() {
                biometric::parse_operlog(&text)
            } else {
                biometric::Parsed::default()
            };
            receive_biometrics(&state, &device, &serial, parsed, stamp).await?;
            Ok(plain(session::render_upload_ok(non_empty_lines(&text))))
        }
        Upload::Biodata => {
            if let Some(stamp) = stamp_of(query.stamp.as_ref()) {
                devices::set_stamp(&state.db, device.id, Stamp::Biodata, stamp).await?;
            }
            Ok(plain(session::render_upload_ok(0)))
        }
        Upload::AttPhoto => {
            let stamp = stamp_of(query.photo_stamp.as_ref()).or(stamp_of(query.stamp.as_ref()));
            if let Some(stamp) = stamp {
                devices::set_stamp(&state.db, device.id, Stamp::Photo, stamp).await?;
            }
            Ok(plain(session::render_upload_ok(0)))
        }
        Upload::AttLog => {
            let summary = ingest::ingest_attlog(
                &state.db,
                device.id,
                zone_of(&device),
                &text,
                stamp_of(query.stamp.as_ref()),
                &state.ingest,
                client_ip(connection.as_ref()).as_deref(),
            )
            .await?;

            tracing::info!(
                serial = %serial,
                baris = summary.lines,
                rusak = summary.malformed,
                kiriman_ulang = summary.resent,
                punch = summary.punched(),
                suspect = summary.suspect,
                hasil = ?summary.outcomes,
                "ATTLOG diterima"
            );
            Ok(plain(session::render_upload_ok(summary.lines)))
        }
    }
}

/// `POST /iclock/fdata`: foto absensi, diabaikan (D7). Balasan `OK\n`.
pub async fn photo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SnQuery>,
) -> Result<Response, AppError> {
    let Some(serial) = serial_of(&headers, query.sn.as_ref()) else {
        return Ok(plain("OK\n"));
    };
    let Some(device) = devices::find_active(&state.db, &serial).await? else {
        tracing::warn!(serial = %serial, "unggahan foto dari mesin tidak terdaftar atau nonaktif ditolak");
        return Ok(empty());
    };
    devices::touch(&state.db, device.id).await?;
    Ok(plain("OK\n"))
}

/// `POST /iclock/registry`: registrasi mesin. Mesin tak dikenal dijawab `UNKNOWN DEVICE`
/// (seperti kode lama); yang dikenal mendapat `RegistryCode` yang tetap.
pub async fn registry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SnQuery>,
    body: Bytes,
) -> Result<Response, AppError> {
    let Some(serial) = serial_of(&headers, query.sn.as_ref()) else {
        return Ok(empty());
    };
    let Some(device) = devices::find_active(&state.db, &serial).await? else {
        tracing::warn!(serial = %serial, "registry dari mesin tidak terdaftar atau nonaktif");
        return Ok(plain("UNKNOWN DEVICE"));
    };
    devices::touch(&state.db, device.id).await?;

    let text = String::from_utf8_lossy(&body);
    let pairs = kv::pairs(&text, true);
    devices::update_info(
        &state.db,
        device.id,
        &DeviceInfo::from_pairs(&pairs),
        Some(&info::bounded_options(&pairs)),
    )
    .await?;
    if let Some(device_type) =
        kv::last(&pairs, "DeviceType").and_then(session::registry_device_type)
    {
        devices::set_device_type(&state.db, device.id, device_type).await?;
    }

    // Kode acak dari CSPRNG (kode lama memakai `rand()` biasa); kode yang sudah ada dipertahankan.
    let candidate = Alphanumeric.sample_string(&mut rand::rng(), 10);
    let code = devices::ensure_registry_code(&state.db, device.id, &candidate).await?;
    Ok(plain(format!("RegistryCode={code}")))
}

/// `POST /iclock/push`: parameter sesi push. Efek samping: meminta pengaturan jam mesin
/// (`set_time`, sekali selama belum ada yang aktif).
pub async fn push(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SnQuery>,
) -> Result<Response, AppError> {
    let Some(serial) = serial_of(&headers, query.sn.as_ref()) else {
        return Ok(empty());
    };
    let Some(device) = devices::find_active(&state.db, &serial).await? else {
        tracing::warn!(serial = %serial, "push dari mesin tidak terdaftar atau nonaktif");
        return Ok(empty());
    };
    devices::touch(&state.db, device.id).await?;
    commands::enqueue_maintenance(&state.db, device.id, CommandKind::SetTime).await?;

    let session_id = Alphanumeric.sample_string(&mut rand::rng(), 10);
    let version = device.push_version.as_deref().unwrap_or("2.3.0");
    Ok(plain(session::render_push(
        version,
        device.error_delay_seconds,
        device.poll_delay_seconds,
        &session_id,
    )))
}

/// `GET /iclock/rtdata?type=time`: jam server dalam detik ZK dan zona mesin.
pub async fn realtime(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RtDataQuery>,
) -> Result<Response, AppError> {
    let Some(serial) = serial_of(&headers, query.sn.as_ref()) else {
        return Ok(empty());
    };
    if query.kind.as_deref() != Some("time") {
        return Ok(empty());
    }
    let Some(device) = devices::find_active(&state.db, &serial).await? else {
        tracing::warn!(serial = %serial, "rtdata dari mesin tidak terdaftar atau nonaktif");
        return Ok(empty());
    };
    devices::touch(&state.db, device.id).await?;

    let now = Utc::now();
    Ok(plain(session::render_rtdata_time(
        zk_time::encode(now),
        zk_time::offset_minutes(zone_of(&device), now),
    )))
}

/// `GET /iclock/ping`: tanpa pemeriksaan apa pun.
pub async fn ping() -> Response {
    plain("OK")
}

/// `POST /iclock/querydata` (A1): `options` dijawab `OK`, `count` memperbarui jumlah data di
/// mesin, `tabledata` hanya dijawab `<tablename>=<count>` tanpa menyimpan (tidak menarik log
/// lama dari mesin).
pub async fn query_data(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<QueryData>,
    body: Bytes,
) -> Result<Response, AppError> {
    let Some(serial) = serial_of(&headers, query.sn.as_ref()) else {
        return Ok(empty());
    };
    let Some(device) = devices::find_active(&state.db, &serial).await? else {
        tracing::warn!(serial = %serial, "querydata dari mesin tidak terdaftar atau nonaktif");
        return Ok(empty());
    };
    devices::touch(&state.db, device.id).await?;

    match query.kind.as_deref() {
        Some("options") => Ok(plain("OK")),
        Some("count") => {
            let pairs = kv::pairs(&String::from_utf8_lossy(&body), false);
            let count = |key: &str| {
                kv::last(&pairs, key)
                    .and_then(|v| v.trim().parse::<i32>().ok())
                    .filter(|n| *n >= 0)
            };
            devices::update_counts(
                &state.db,
                device.id,
                count("user"),
                count("templatev10"),
                count("biophoto"),
            )
            .await?;
            Ok(plain("OK"))
        }
        Some("tabledata") => match (query.tablename.as_deref(), query.count.as_deref()) {
            (Some(name), Some(count)) if is_token(name) && is_token(count) => {
                Ok(plain(format!("{name}={count}")))
            }
            _ => Ok(empty()),
        },
        _ => Ok(empty()),
    }
}

/// Nama tabel dan jumlah dipantulkan ke balasan; hanya karakter aman yang boleh.
fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Menyimpan template dari satu unggahan (bila brankas aktif) dan stamp-nya di transaksi yang sama.
/// Tanpa brankas, isi template dibuang (tidak pernah disimpan tanpa enkripsi) dan hanya stamp yang
/// disimpan. Log hanya memuat jumlah.
async fn receive_biometrics(
    state: &AppState,
    device: &devices::Device,
    serial: &str,
    parsed: biometric::Parsed,
    stamp: Option<(Stamp, &str)>,
) -> Result<(), AppError> {
    let biometric::Parsed { rows, skipped, .. } = parsed;

    let Some(vault) = state.biometrics.vault.as_ref() else {
        if let Some((kind, value)) = stamp {
            devices::set_stamp(&state.db, device.id, kind, value).await?;
        }
        return Ok(());
    };
    if rows.is_empty() && stamp.is_none() {
        return Ok(());
    }

    let received = rows.len();
    let summary = bio_ingest::ingest_templates(
        &state.db,
        device,
        vault,
        state.biometrics.store_face_photo,
        rows,
        stamp,
    )
    .await?;

    if received > 0 || skipped > 0 {
        tracing::info!(
            serial = %serial,
            diterima = received,
            tersimpan = summary.stored,
            tidak_berubah = summary.unchanged,
            pin_tak_dikenal = summary.unknown_pin,
            foto_diabaikan = summary.photos_ignored,
            dilewati = skipped,
            "template biometrik diproses"
        );
    }
    Ok(())
}
