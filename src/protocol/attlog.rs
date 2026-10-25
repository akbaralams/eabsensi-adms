//! Penguraian badan `POST /iclock/cdata` dan pengelompokan jenis unggahannya.

use chrono::{DateTime, LocalResult, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

/// Format waktu log di badan ATTLOG dan di konfigurasi.
pub const TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// Panjang maksimum PIN yang bisa disimpan (`device_logs.pin` adalah `varchar(32)`).
pub const MAX_PIN_CHARS: usize = 32;

/// Jenis unggahan `POST /iclock/cdata`, menurut urutan percabangan `idata` lama.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upload {
    /// `table=rtstate`: status realtime, tidak diproses.
    RtState,
    /// `table=options`: laporan opsi perangkat.
    Options,
    /// `table=rtlog`: log realtime (diabaikan, D7).
    RtLog,
    /// `table=tabledata`: data tabel (biometrik, ditangani di Task 5b; sementara diabaikan).
    TableData,
    /// `OPERLOG`/`USERINFO`/`FINGERTMP` atau ada `OpStamp` (OPLOG diabaikan, D6).
    Operation,
    /// `table=BIODATA`.
    Biodata,
    /// `table=ATTLOG` atau ada `Stamp`: log absensi.
    AttLog,
    /// `table=ATTPHOTO` atau ada `PhotoStamp` (foto diabaikan, D7).
    AttPhoto,
    /// Selain itu: dibalas `OK`.
    Unknown,
}

/// Urutan pemeriksaan sama dengan `idata` lama (`docs/legacy-analysis.md` bagian 3.3).
pub fn classify(
    table: Option<&str>,
    has_opstamp: bool,
    has_stamp: bool,
    has_photostamp: bool,
) -> Upload {
    match table {
        Some("rtstate") => return Upload::RtState,
        Some("options") => return Upload::Options,
        Some("rtlog") => return Upload::RtLog,
        Some("tabledata") => return Upload::TableData,
        _ => {}
    }

    let operation_table = matches!(table, Some("OPERLOG" | "USERINFO" | "FINGERTMP"));
    if operation_table || has_opstamp {
        Upload::Operation
    } else if table == Some("BIODATA") {
        Upload::Biodata
    } else if table == Some("ATTLOG") || (has_stamp && !matches!(table, Some("ATTPHOTO"))) {
        Upload::AttLog
    } else if table == Some("ATTPHOTO") || has_photostamp {
        Upload::AttPhoto
    } else {
        Upload::Unknown
    }
}

/// Satu baris log absensi yang sah.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttRow {
    pub pin: String,
    /// Waktu lokal mesin persis seperti dikirim.
    pub device_time: NaiveDateTime,
    pub io_status: Option<i16>,
    pub verify_mode: Option<i16>,
}

/// Hasil penguraian satu badan ATTLOG.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ParsedBatch {
    pub rows: Vec<AttRow>,
    /// Jumlah baris tak kosong (sah atau tidak), dipakai untuk balasan `OK:<n>`.
    pub lines: usize,
    /// Alasan tiap baris yang tidak bisa dibaca; baris itu dilewati, tidak menggagalkan batch.
    pub malformed: Vec<Malformed>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed {
    /// Nomor baris (mulai 1) di badan.
    pub line: usize,
    pub reason: &'static str,
}

/// Format baris: `PIN \t waktu \t status \t verifikasi \t ...` (kolom setelahnya diabaikan).
/// PIN dipakai apa adanya (tanpa trim dan tanpa padding, keputusan D9).
pub fn parse_attlog(body: &str) -> ParsedBatch {
    let mut batch = ParsedBatch::default();

    for (index, raw) in body.split('\n').enumerate() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        batch.lines += 1;

        let mut fields = line.split('\t');
        let pin = fields.next().unwrap_or_default();
        let Some(time) = fields.next() else {
            batch.malformed.push(Malformed {
                line: index + 1,
                reason: "kolom waktu tidak ada",
            });
            continue;
        };

        if pin.is_empty() {
            batch.malformed.push(Malformed {
                line: index + 1,
                reason: "PIN kosong",
            });
            continue;
        }
        if pin.chars().count() > MAX_PIN_CHARS {
            batch.malformed.push(Malformed {
                line: index + 1,
                reason: "PIN terlalu panjang",
            });
            continue;
        }
        let Ok(device_time) = NaiveDateTime::parse_from_str(time.trim(), TIME_FORMAT) else {
            batch.malformed.push(Malformed {
                line: index + 1,
                reason: "format waktu tidak dikenal",
            });
            continue;
        };

        let number = |field: Option<&str>| field.and_then(|v| v.trim().parse::<i16>().ok());
        let io_status = number(fields.next());
        let verify_mode = number(fields.next());

        batch.rows.push(AttRow {
            pin: pin.to_owned(),
            device_time,
            io_status,
            verify_mode,
        });
    }

    batch
}

/// Mengubah waktu lokal mesin ke UTC memakai zona mesin. Waktu yang tidak ada (celah DST) tidak
/// dapat diubah; waktu ganda (DST mundur) memakai kemunculan pertama.
pub fn to_utc(device_time: NaiveDateTime, zone: Tz) -> Option<DateTime<Utc>> {
    match zone.from_local_datetime(&device_time) {
        LocalResult::Single(local) => Some(local.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc)),
        LocalResult::None => None,
    }
}

/// Stamp yang boleh disimpan ke `devices.*_stamp`: angka, panjang wajar. Selain itu diabaikan
/// agar nilai aneh dari mesin tidak merusak handshake berikutnya.
pub fn valid_stamp(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 20 && value.bytes().all(|b| b.is_ascii_digit()))
        .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(value: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(value, TIME_FORMAT).unwrap()
    }

    #[test]
    fn klasifikasi_mengikuti_urutan_idata_lama() {
        use Upload::*;
        assert_eq!(classify(Some("rtstate"), false, false, false), RtState);
        assert_eq!(classify(Some("options"), false, false, false), Options);
        assert_eq!(classify(Some("rtlog"), false, false, false), RtLog);
        assert_eq!(classify(Some("tabledata"), false, false, false), TableData);
        // Dari log produksi: OPERLOG dengan OpStamp, dan ATTLOG dengan Stamp.
        assert_eq!(classify(Some("OPERLOG"), true, false, false), Operation);
        assert_eq!(classify(Some("ATTLOG"), false, true, false), AttLog);
        assert_eq!(classify(Some("USERINFO"), false, false, false), Operation);
        assert_eq!(classify(Some("FINGERTMP"), false, false, false), Operation);
        // OpStamp tanpa table tetap operasi (cabang lama `|| has('OpStamp')`).
        assert_eq!(classify(None, true, false, false), Operation);
        assert_eq!(classify(Some("BIODATA"), false, true, false), Biodata);
        // Stamp tanpa table dianggap ATTLOG, kecuali table ATTPHOTO.
        assert_eq!(classify(None, false, true, false), AttLog);
        assert_eq!(classify(Some("ATTPHOTO"), false, true, false), AttPhoto);
        assert_eq!(classify(Some("ATTPHOTO"), false, false, false), AttPhoto);
        assert_eq!(classify(None, false, false, true), AttPhoto);
        assert_eq!(classify(None, false, false, false), Unknown);
        assert_eq!(classify(Some("lain"), false, false, false), Unknown);
    }

    #[test]
    fn baris_attlog_lengkap() {
        let batch = parse_attlog(
            "12345\t2024-06-05 07:01:03\t0\t1\t0\t0\t0\n12346\t2024-06-05 07:02:41\t1\t15\n",
        );

        assert_eq!(batch.lines, 2);
        assert!(batch.malformed.is_empty());
        assert_eq!(
            batch.rows,
            vec![
                AttRow {
                    pin: "12345".to_owned(),
                    device_time: time("2024-06-05 07:01:03"),
                    io_status: Some(0),
                    verify_mode: Some(1),
                },
                AttRow {
                    pin: "12346".to_owned(),
                    device_time: time("2024-06-05 07:02:41"),
                    io_status: Some(1),
                    verify_mode: Some(15),
                },
            ]
        );
    }

    #[test]
    fn crlf_baris_kosong_dan_kolom_pendek_ditoleransi() {
        let batch = parse_attlog("7\t2024-06-05 07:01:03\r\n\r\n8\t2024-06-05 07:02:00\t1\n\n");

        assert_eq!(batch.lines, 2, "baris kosong tidak dihitung");
        assert_eq!(batch.rows.len(), 2);
        assert_eq!(batch.rows[0].io_status, None);
        assert_eq!(batch.rows[0].verify_mode, None);
        assert_eq!(batch.rows[1].io_status, Some(1));
        assert_eq!(batch.rows[1].verify_mode, None);
    }

    #[test]
    fn pin_dipakai_apa_adanya_tanpa_trim_dan_padding() {
        let batch = parse_attlog("0123\t2024-06-05 07:01:03\nE100 \t2024-06-05 07:01:04\n");

        assert_eq!(batch.rows[0].pin, "0123");
        assert_eq!(batch.rows[1].pin, "E100 ");
    }

    #[test]
    fn baris_rusak_dilewati_dengan_alasan_dan_tidak_menggagalkan_batch() {
        let batch = parse_attlog(
            "1\n\t2024-06-05 07:00:00\n2\tkemarin\n3\t2024-13-40 25:61:61\n4\t2024-06-05 07:00:00\n",
        );

        assert_eq!(batch.lines, 5);
        assert_eq!(batch.rows.len(), 1);
        assert_eq!(batch.rows[0].pin, "4");
        let reasons: Vec<_> = batch.malformed.iter().map(|m| (m.line, m.reason)).collect();
        assert_eq!(
            reasons,
            vec![
                (1, "kolom waktu tidak ada"),
                (2, "PIN kosong"),
                (3, "format waktu tidak dikenal"),
                (4, "format waktu tidak dikenal"),
            ]
        );
    }

    #[test]
    fn pin_terlalu_panjang_ditolak() {
        let panjang = "9".repeat(33);
        let batch = parse_attlog(&format!("{panjang}\t2024-06-05 07:00:00\n"));

        assert!(batch.rows.is_empty());
        assert_eq!(batch.malformed[0].reason, "PIN terlalu panjang");
        let tepat = "9".repeat(32);
        assert_eq!(
            parse_attlog(&format!("{tepat}\t2024-06-05 07:00:00\n"))
                .rows
                .len(),
            1
        );
    }

    #[test]
    fn badan_kosong() {
        assert_eq!(parse_attlog(""), ParsedBatch::default());
        assert_eq!(parse_attlog("\n\r\n  \n"), ParsedBatch::default());
    }

    #[test]
    fn konversi_zona_ke_utc() {
        let jakarta = to_utc(time("2026-08-03 07:55:00"), chrono_tz::Asia::Jakarta).unwrap();
        assert_eq!(
            jakarta.format(TIME_FORMAT).to_string(),
            "2026-08-03 00:55:00"
        );

        let makassar = to_utc(time("2026-08-03 08:00:00"), chrono_tz::Asia::Makassar).unwrap();
        assert_eq!(
            makassar.format(TIME_FORMAT).to_string(),
            "2026-08-03 00:00:00"
        );
    }

    #[test]
    fn celah_dan_kembar_dst() {
        // New York 2026-03-08 02:30 tidak ada; 2026-11-01 01:30 terjadi dua kali.
        let ny = chrono_tz::America::New_York;
        assert_eq!(to_utc(time("2026-03-08 02:30:00"), ny), None);
        let first = to_utc(time("2026-11-01 01:30:00"), ny).unwrap();
        assert_eq!(first.format(TIME_FORMAT).to_string(), "2026-11-01 05:30:00");
    }

    #[test]
    fn stamp_valid_hanya_angka_pendek() {
        assert_eq!(valid_stamp("785168153"), Some("785168153"));
        assert_eq!(valid_stamp(" 42 "), Some("42"));
        assert_eq!(valid_stamp(""), None);
        assert_eq!(valid_stamp("None"), None);
        assert_eq!(valid_stamp("12;DROP"), None);
        assert_eq!(valid_stamp(&"1".repeat(21)), None);
    }
}
