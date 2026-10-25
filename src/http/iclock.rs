//! Endpoint `/iclock/*` untuk mesin: handshake, polling perintah, dan hasil perintah.
//!
//! Perilaku mengikuti `docs/legacy-analysis.md` bagian 3, dengan perbedaan yang diputuskan di
//! `docs/schema-proposal.md`: mesin harus terdaftar dan aktif di semua endpoint (D2), heartbeat
//! lewat `devices.last_seen_at`, dan antrean di `device_commands`.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use chrono_tz::Tz;
use serde::Deserialize;

use crate::db::{commands, devices};
use crate::protocol::command::{self, CommandKind, DeviceCmdBody, RenderError, User};
use crate::protocol::handshake::{self, HandshakeDevice, Stamps};
use crate::protocol::info::{self, DeviceInfo};
use crate::protocol::zk_time;
use crate::state::AppState;

/// `Content-Type` yang dikirim kode lama (Symfony menambahkan charset pada `text/*`).
const PLAIN: &str = "text/plain; charset=UTF-8";
/// Kode lama membalas `null` (body kosong, `text/html`) untuk permintaan yang ditolak.
const EMPTY: &str = "text/html; charset=UTF-8";

const DEFAULT_ZONE: Tz = chrono_tz::Asia::Jakarta;

#[derive(Debug, Deserialize)]
pub struct DeviceQuery {
    #[serde(rename = "SN")]
    sn: Option<String>,
    options: Option<String>,
    pushver: Option<String>,
    #[serde(rename = "PushOptionsFlag")]
    push_options_flag: Option<String>,
    #[serde(rename = "DeviceType")]
    device_type: Option<String>,
    #[serde(rename = "INFO")]
    info: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SnQuery {
    #[serde(rename = "SN")]
    sn: Option<String>,
}

/// Galat internal: dicatat, dijawab 500 tanpa rincian ke mesin (mesin akan mencoba lagi setelah
/// `ErrorDelay`).
pub struct AppError;

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(%error, "galat database saat melayani mesin");
        Self
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

fn reply(content_type: &'static str, body: impl Into<String>) -> Response {
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static(content_type))],
        body.into(),
    )
        .into_response()
}

fn plain(body: impl Into<String>) -> Response {
    reply(PLAIN, body)
}

fn empty() -> Response {
    reply(EMPTY, "")
}

/// Kode lama hanya melayani klien dengan `User-Agent` mengandung `iClock`.
fn is_iclock(headers: &HeaderMap) -> bool {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|agent| agent.contains("iClock"))
}

/// Nomor seri dari query bila request tampak datang dari mesin.
fn serial_of(headers: &HeaderMap, sn: Option<&String>) -> Option<String> {
    let serial = sn
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())?;
    is_iclock(headers).then(|| serial.to_owned())
}

fn zone_of(device: &devices::Device) -> Tz {
    device.timezone.parse().unwrap_or_else(|_| {
        tracing::warn!(serial = %device.serial_number, zona = %device.timezone, "zona waktu mesin tidak valid, memakai Asia/Jakarta");
        DEFAULT_ZONE
    })
}

/// `GET /iclock/cdata`: handshake. Mesin tak terdaftar/nonaktif atau request tak lengkap dijawab
/// kosong (seperti kode lama), tanpa efek apa pun.
pub async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DeviceQuery>,
) -> Result<Response, AppError> {
    let Some(serial) = serial_of(&headers, query.sn.as_ref()) else {
        return Ok(empty());
    };
    if query.options.is_none() {
        return Ok(empty());
    }

    let Some(device) = devices::find_active(&state.db, &serial).await? else {
        tracing::warn!(serial = %serial, "handshake dari mesin tidak terdaftar atau nonaktif");
        return Ok(empty());
    };
    devices::touch(&state.db, device.id).await?;

    if handshake::is_push3(query.pushver.as_deref(), query.push_options_flag.as_deref()) {
        let pushver = query.pushver.as_deref().unwrap_or_default();
        devices::set_push_version(&state.db, device.id, pushver).await?;
        return Ok(plain(handshake::render_push3(pushver)));
    }

    let requested_att = matches!(query.device_type.as_deref(), Some("att" | "0"));
    let mut stored_is_att = device.device_type == "att";

    if !device.initialized {
        // Mesin baru: minta INFO dulu; hasilnya menandai mesin terinisialisasi dan menyusul CHECK.
        if requested_att {
            devices::mark_att(&state.db, device.id).await?;
            stored_is_att = true;
        }
        commands::enqueue_maintenance(&state.db, device.id, CommandKind::Info).await?;
    }

    let format = handshake::select_format(
        device.initialized,
        device.push_version.as_deref(),
        stored_is_att,
        query.device_type.as_deref(),
    );
    let body = handshake::render(
        format,
        &HandshakeDevice {
            serial: device.serial_number.clone(),
            stamps: Stamps {
                attlog: device.attlog_stamp.clone(),
                operlog: device.operlog_stamp.clone(),
                photo: device.photo_stamp.clone(),
                biodata: device.biodata_stamp.clone(),
            },
            poll_delay_seconds: device.poll_delay_seconds,
            error_delay_seconds: device.error_delay_seconds,
            offset_minutes: zk_time::offset_minutes(zone_of(&device), Utc::now()),
        },
    );
    Ok(plain(body))
}

/// `GET /iclock/getrequest`: heartbeat dan pengambilan perintah tertunda. Tanpa perintah, `OK`.
pub async fn poll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DeviceQuery>,
) -> Result<Response, AppError> {
    let Some(serial) = serial_of(&headers, query.sn.as_ref()) else {
        return Ok(plain("OK"));
    };
    let Some(device) = devices::find_active(&state.db, &serial).await? else {
        tracing::warn!(serial = %serial, "polling dari mesin tidak terdaftar atau nonaktif");
        return Ok(plain("OK"));
    };
    devices::touch(&state.db, device.id).await?;

    // Laporan info (`INFO=...`) menggantikan polling perintah pada request itu.
    if let Some(raw) = query.info.as_deref() {
        devices::update_info(
            &state.db,
            device.id,
            &DeviceInfo::from_info_param(raw),
            None,
        )
        .await?;
        return Ok(plain("OK"));
    }

    commands::expire_unanswered(
        &state.db,
        device.id,
        state.queue.ack_timeout,
        state.queue.max_attempts,
    )
    .await?;

    let claimed = commands::claim_batch(&state.db, device.id).await?;
    if claimed.is_empty() {
        return Ok(plain("OK"));
    }

    let employee_ids: Vec<i64> = claimed.iter().filter_map(|c| c.employee_id).collect();
    let employees: HashMap<i64, commands::EmployeeRow> = if employee_ids.is_empty() {
        HashMap::new()
    } else {
        commands::employees_by_id(&state.db, &employee_ids)
            .await?
            .into_iter()
            .map(|employee| (employee.id, employee))
            .collect()
    };

    let offset_minutes = zk_time::offset_minutes(zone_of(&device), Utc::now());
    let zk_now = zk_time::encode(Utc::now());

    let mut body = String::new();
    for item in &claimed {
        let rendered = match CommandKind::from_db(&item.command_type) {
            None => Err(RenderError::Unsupported),
            Some(kind) => command::render(&command::Render {
                id: item.id,
                kind,
                offset_minutes,
                zk_now,
                user: item
                    .employee_id
                    .and_then(|id| employees.get(&id))
                    .map(|employee| User {
                        pin: &employee.employee_number,
                        name: &employee.name,
                    }),
            }),
        };

        match rendered {
            Ok(lines) => body.push_str(&lines),
            Err(error) => {
                tracing::warn!(serial = %serial, perintah = item.id, jenis = %item.command_type, %error, "perintah tidak dapat dirakit");
                commands::fail(&state.db, item.id, &error.to_string()).await?;
            }
        }
    }

    Ok(plain(if body.is_empty() {
        "OK".to_owned()
    } else {
        body
    }))
}

/// `POST /iclock/devicecmd`: hasil perintah. Selalu dijawab `OK\n`.
pub async fn command_result(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SnQuery>,
    body: Bytes,
) -> Result<Response, AppError> {
    let ok = || plain("OK\n");

    let Some(serial) = serial_of(&headers, query.sn.as_ref()) else {
        return Ok(ok());
    };
    let Some(device) = devices::find_active(&state.db, &serial).await? else {
        tracing::warn!(serial = %serial, "hasil perintah dari mesin tidak terdaftar atau nonaktif");
        return Ok(ok());
    };
    devices::touch(&state.db, device.id).await?;

    let text = String::from_utf8_lossy(&body);
    match command::parse_devicecmd(&text) {
        DeviceCmdBody::Info { result, pairs } => {
            let options = info::bounded_options(&pairs);
            devices::update_info(
                &state.db,
                device.id,
                &DeviceInfo::from_pairs(&pairs),
                Some(&options),
            )
            .await?;
            commands::complete(&state.db, device.id, result.id, result.return_code).await?;

            // INFO sukses = mesin siap: tandai terinisialisasi dan minta CHECK (kode lama).
            if result.is_success() && !device.initialized {
                devices::mark_initialized(&state.db, device.id).await?;
                commands::enqueue_maintenance(&state.db, device.id, CommandKind::Check).await?;
            }
        }
        DeviceCmdBody::Results(results) => {
            for result in results {
                let updated =
                    commands::complete(&state.db, device.id, result.id, result.return_code).await?;
                if !updated {
                    tracing::debug!(serial = %serial, perintah = result.id, "jawaban untuk perintah yang tidak aktif diabaikan");
                }
            }
        }
    }

    Ok(ok())
}
