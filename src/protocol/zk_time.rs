//! Waktu dan zona dalam bentuk yang dipakai firmware ZKTeco.

use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;

/// Detik ZK: bulan dianggap 31 hari, tahun dihitung dari 2000. Bukan epoch Unix
/// (`docs/legacy-analysis.md` bagian 2).
pub fn encode(at: DateTime<Utc>) -> i64 {
    let days = (i64::from(at.year()) - 2000) * 12 * 31
        + i64::from(at.month0()) * 31
        + i64::from(at.day0());
    days * 86_400
        + i64::from(at.hour()) * 3_600
        + i64::from(at.minute()) * 60
        + i64::from(at.second())
}

/// Selisih zona terhadap UTC dalam menit pada saat `at`. Zona yang tak dikenal dianggap UTC+0
/// oleh pemanggil; di sini hanya zona valid.
pub fn offset_minutes(zone: Tz, at: DateTime<Utc>) -> i32 {
    use chrono::{Offset, TimeZone};
    zone.offset_from_utc_datetime(&at.naive_utc())
        .fix()
        .local_minus_utc()
        / 60
}

/// Nilai `TimeZone=` di handshake: jam, bilangan bulat bila pas ("7"), desimal bila tidak ("5.5").
pub fn hours_label(offset_minutes: i32) -> String {
    if offset_minutes % 60 == 0 {
        (offset_minutes / 60).to_string()
    } else {
        format!("{}", f64::from(offset_minutes) / 60.0)
    }
}

/// Bentuk `±HHMM` untuk `MachineTZ=` dan `ServerTZ=`.
pub fn hhmm(offset_minutes: i32) -> String {
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let abs = offset_minutes.abs();
    format!("{sign}{:02}{:02}", abs / 60, abs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn encode_cocok_dengan_stamp_nyata_dari_log_produksi() {
        // 5 Jun 2024 00:00 UTC = hari ke-9087 (analisis lama bagian 2.1).
        assert_eq!(encode(utc(2024, 6, 5, 0, 0, 0)), 9087 * 86_400);
        // 16:13:12 WIB = 09:13:12 UTC.
        assert_eq!(encode(utc(2024, 6, 5, 9, 13, 12)), 785_149_992);
        // Stamp ATTLOG 785168153 dari log = 5 Jun 2024 14:15:53 pada pengodean ZK.
        assert_eq!(encode(utc(2024, 6, 5, 14, 15, 53)), 785_168_153);
    }

    #[test]
    fn encode_memakai_bulan_31_hari() {
        assert_eq!(encode(utc(2000, 1, 1, 0, 0, 0)), 0);
        assert_eq!(encode(utc(2000, 2, 1, 0, 0, 0)), 31 * 86_400);
        assert_eq!(encode(utc(2001, 1, 1, 0, 0, 0)), 12 * 31 * 86_400);
    }

    #[test]
    fn offset_zona_indonesia() {
        let now = utc(2026, 10, 26, 0, 0, 0);
        assert_eq!(offset_minutes(chrono_tz::Asia::Jakarta, now), 420);
        assert_eq!(offset_minutes(chrono_tz::Asia::Makassar, now), 480);
        assert_eq!(offset_minutes(chrono_tz::Asia::Jayapura, now), 540);
        assert_eq!(offset_minutes(chrono_tz::Asia::Kolkata, now), 330);
    }

    #[test]
    fn label_jam_bulat_dan_pecahan() {
        assert_eq!(hours_label(420), "7");
        assert_eq!(hours_label(0), "0");
        assert_eq!(hours_label(330), "5.5");
        assert_eq!(hours_label(345), "5.75");
        assert_eq!(hours_label(-210), "-3.5");
        assert_eq!(hours_label(-300), "-5");
    }

    #[test]
    fn hhmm_dengan_tanda() {
        assert_eq!(hhmm(420), "+0700");
        assert_eq!(hhmm(330), "+0530");
        assert_eq!(hhmm(-210), "-0330");
        assert_eq!(hhmm(0), "+0000");
    }
}
