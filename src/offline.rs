//! Pemantauan mesin offline untuk operasional (`eabsensi-adms check-offline`).
//!
//! Mesin dianggap offline bila tidak terlihat (`devices.last_seen_at`) lebih lama dari
//! `max(faktor x poll_delay_seconds, batas_minimum)`. Mesin nonaktif tidak dihitung (operator
//! sengaja mematikannya). UI v2 memakai faktor 3 tanpa batas minimum; batas minimum di sini
//! (bawaan 120 detik) mencegah peringatan berkedip untuk mesin berjeda polling sangat pendek.

use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq)]
pub struct OfflineDevice {
    pub serial: String,
    pub name: String,
    /// Detik sejak terakhir terlihat; `None` bila belum pernah terhubung.
    pub seconds_since_seen: Option<f64>,
    /// Batas yang dipakai untuk mesin ini (detik).
    pub threshold_seconds: f64,
}

pub async fn find(
    pool: &PgPool,
    factor: u32,
    min_seconds: u32,
) -> sqlx::Result<Vec<OfflineDevice>> {
    let rows = sqlx::query!(
        r#"SELECT serial_number,
                  name,
                  EXTRACT(EPOCH FROM (now() - last_seen_at))::float8 AS seconds_since_seen,
                  GREATEST($1::float8 * poll_delay_seconds, $2::float8) AS "threshold!"
             FROM devices
            WHERE is_active
              AND (last_seen_at IS NULL
                   OR last_seen_at < now() - make_interval(
                          secs => GREATEST($1::float8 * poll_delay_seconds, $2::float8)))
            ORDER BY last_seen_at NULLS FIRST, serial_number"#,
        f64::from(factor),
        f64::from(min_seconds)
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| OfflineDevice {
            serial: row.serial_number,
            name: row.name,
            seconds_since_seen: row.seconds_since_seen,
            threshold_seconds: row.threshold,
        })
        .collect())
}

/// "45 detik", "5 menit", "3 jam 20 menit", "2 hari 4 jam".
pub fn human_duration(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (days, hours, minutes) = (
        total / 86_400,
        (total % 86_400) / 3_600,
        (total % 3_600) / 60,
    );

    let join = |major: u64, major_unit: &str, minor: u64, minor_unit: &str| {
        if minor == 0 {
            format!("{major} {major_unit}")
        } else {
            format!("{major} {major_unit} {minor} {minor_unit}")
        }
    };

    if days > 0 {
        join(days, "hari", hours, "jam")
    } else if hours > 0 {
        join(hours, "jam", minutes, "menit")
    } else if minutes > 0 {
        format!("{minutes} menit")
    } else {
        format!("{total} detik")
    }
}

/// Teks untuk operator atau penjadwal: satu baris per mesin offline, atau satu baris "semua online".
pub fn render(devices: &[OfflineDevice]) -> String {
    if devices.is_empty() {
        return "OK semua mesin aktif online\n".to_owned();
    }

    let mut out = format!("{} mesin aktif offline\n", devices.len());
    for device in devices {
        let seen = match device.seconds_since_seen {
            Some(seconds) => format!("terakhir terlihat {} lalu", human_duration(seconds)),
            None => "belum pernah terhubung".to_owned(),
        };
        out.push_str(&format!(
            "OFFLINE {} \"{}\" {seen} (batas {})\n",
            device.serial,
            device.name,
            human_duration(device.threshold_seconds)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durasi_dalam_bahasa_operator() {
        assert_eq!(human_duration(0.0), "0 detik");
        assert_eq!(human_duration(45.9), "45 detik");
        assert_eq!(human_duration(60.0), "1 menit");
        assert_eq!(human_duration(300.0), "5 menit");
        assert_eq!(human_duration(3_600.0), "1 jam");
        assert_eq!(human_duration(12_000.0), "3 jam 20 menit");
        assert_eq!(human_duration(86_400.0), "1 hari");
        assert_eq!(
            human_duration(2.0 * 86_400.0 + 4.0 * 3_600.0 + 59.0),
            "2 hari 4 jam"
        );
        assert_eq!(human_duration(-5.0), "0 detik");
    }

    #[test]
    fn keluaran_untuk_semua_online_dan_ada_yang_offline() {
        assert_eq!(render(&[]), "OK semua mesin aktif online\n");

        let text = render(&[
            OfflineDevice {
                serial: "SN-1".to_owned(),
                name: "Lobi".to_owned(),
                seconds_since_seen: Some(700.0),
                threshold_seconds: 180.0,
            },
            OfflineDevice {
                serial: "SN-2".to_owned(),
                name: "Gudang".to_owned(),
                seconds_since_seen: None,
                threshold_seconds: 180.0,
            },
        ]);

        assert_eq!(
            text,
            "2 mesin aktif offline\n\
             OFFLINE SN-1 \"Lobi\" terakhir terlihat 11 menit lalu (batas 3 menit)\n\
             OFFLINE SN-2 \"Gudang\" belum pernah terhubung (batas 3 menit)\n"
        );
    }
}
