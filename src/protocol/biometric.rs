//! Baris template biometrik dari mesin: `FP`/`FACE`/`BIOPHOTO` di badan `OPERLOG`, dan
//! `templatev10`/`biophoto` di `table=tabledata`. Isi template tidak pernah ikut tercetak
//! (`Debug` menyembunyikan data) dan tidak pernah ditulis ke log.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::attlog::MAX_PIN_CHARS;

/// Batas ukuran satu template sidik jari atau wajah (byte setelah dekode base64).
pub const MAX_TEMPLATE_BYTES: usize = 64 * 1024;
/// Batas ukuran satu foto wajah (byte setelah dekode base64).
pub const MAX_PHOTO_BYTES: usize = 512 * 1024;
/// Nomor jari/indeks yang masih masuk akal (`finger_index` bertipe `smallint`).
pub const MAX_FINGER_INDEX: i16 = 127;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fingerprint,
    Face,
    FacePhoto,
}

impl Kind {
    /// Nilai kolom `biometric_templates.kind` (`BiometricKind` di eabsensi-v2).
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Fingerprint => "fingerprint",
            Self::Face => "face",
            Self::FacePhoto => "face_photo",
        }
    }
}

/// Satu template dari mesin. `data` adalah isi biner (hasil dekode base64).
#[derive(Clone, PartialEq, Eq)]
pub struct Row {
    pub pin: String,
    pub kind: Kind,
    pub finger_index: i16,
    /// `Size` yang dilaporkan mesin; bila tidak ada dipakai panjang `data`.
    pub size: i32,
    pub valid: bool,
    pub data: Vec<u8>,
}

impl fmt::Debug for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Row")
            .field("kind", &self.kind)
            .field("finger_index", &self.finger_index)
            .field("data_len", &self.data.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub rows: Vec<Row>,
    /// Baris biometrik yang tidak bisa dibaca atau terlalu besar (dilewati).
    pub skipped: usize,
    /// Baris tak kosong yang bukan biometrik (OPLOG, USER, ...): diabaikan.
    pub other: usize,
}

/// Badan `table=OPERLOG`: `FP `, `FACE `, `BIOPHOTO ` diambil; `OPLOG`, `USER`, dst. diabaikan (D6).
pub fn parse_operlog(body: &str) -> Parsed {
    let mut parsed = Parsed::default();
    for line in lines(body) {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let kind = match word {
            "FP" => Kind::Fingerprint,
            "FACE" => Kind::Face,
            "BIOPHOTO" => Kind::FacePhoto,
            _ => {
                parsed.other += 1;
                continue;
            }
        };
        push(&mut parsed, row(kind, rest));
    }
    parsed
}

/// Badan `table=tabledata`; hanya `templatev10` (sidik jari) dan `biophoto` (foto wajah).
pub fn parse_tabledata(tablename: &str, body: &str) -> Parsed {
    let kind = match tablename {
        "templatev10" => Kind::Fingerprint,
        "biophoto" => Kind::FacePhoto,
        _ => return Parsed::default(),
    };
    let mut parsed = Parsed::default();
    for line in lines(body) {
        // Awalan nama tabel (`templatev10 pin=...`) dibuang.
        let rest = line.split_once(' ').map_or(line, |(_, rest)| rest);
        push(&mut parsed, row(kind, rest));
    }
    parsed
}

fn lines(body: &str) -> impl Iterator<Item = &str> {
    body.split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
}

fn push(parsed: &mut Parsed, row: Option<Row>) {
    match row {
        Some(row) => parsed.rows.push(row),
        None => parsed.skipped += 1,
    }
}

/// Kunci tidak membedakan huruf besar/kecil (`Size` di `FP`, `SIZE` di `FACE`, `size` di tabledata).
fn field<'a>(fields: &'a [(String, &'a str)], names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|name| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| *value)
    })
}

fn row(kind: Kind, rest: &str) -> Option<Row> {
    let fields: Vec<(String, &str)> = rest
        .split('\t')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value))
        .collect();

    let pin = field(&fields, &["pin"])?.trim_end();
    if pin.is_empty() || pin.chars().count() > MAX_PIN_CHARS {
        return None;
    }

    let finger_index = match kind {
        Kind::Fingerprint => field(&fields, &["fid", "fingerid"])?
            .trim()
            .parse::<i16>()
            .ok()?,
        Kind::Face => field(&fields, &["fid"])
            .and_then(|v| v.trim().parse::<i16>().ok())
            .unwrap_or(0),
        Kind::FacePhoto => 0,
    };
    if !(0..=MAX_FINGER_INDEX).contains(&finger_index) {
        return None;
    }

    let encoded = match kind {
        Kind::FacePhoto => field(&fields, &["content"])?,
        _ => field(&fields, &["tmp", "template"])?,
    };
    // Mesin mengirim '+' sebagai spasi (kode lama: `str_replace(" ", "+")`), termasuk di awal dan
    // akhir nilai, jadi spasi tidak boleh dipangkas sebelum diganti.
    let data = STANDARD.decode(encoded.replace(' ', "+")).ok()?;
    let limit = if kind == Kind::FacePhoto {
        MAX_PHOTO_BYTES
    } else {
        MAX_TEMPLATE_BYTES
    };
    if data.is_empty() || data.len() > limit {
        return None;
    }

    let size = field(&fields, &["size"])
        .and_then(|v| v.trim().parse::<i32>().ok())
        .filter(|n| *n > 0)
        .or_else(|| i32::try_from(data.len()).ok())?;
    let valid = field(&fields, &["valid"]).is_none_or(|v| v.trim() != "0");

    Some(Row {
        pin: pin.to_owned(),
        kind,
        finger_index,
        size,
        valid,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // "AQIDBAUG" = bytes 1..=6.
    const TMP: &str = "AQIDBAUG";

    fn bytes() -> Vec<u8> {
        vec![1, 2, 3, 4, 5, 6]
    }

    #[test]
    fn baris_fp_dan_face_di_operlog_dan_sisanya_diabaikan() {
        let body = format!(
            "OPLOG 4\t0\t2024-06-05 10:27:20\t0\t0\t0\t0\n\
             USER PIN=7\tName=Budi\tPri=0\n\
             FP PIN=7\tFID=2\tSize=6\tValid=1\tTMP={TMP}\n\
             FACE PIN=7\tFID=0\tSIZE=6\tVALID=1\tTMP={TMP}\n\
             \n"
        );

        let parsed = parse_operlog(&body);

        assert_eq!(parsed.other, 2);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(
            parsed.rows,
            vec![
                Row {
                    pin: "7".to_owned(),
                    kind: Kind::Fingerprint,
                    finger_index: 2,
                    size: 6,
                    valid: true,
                    data: bytes()
                },
                Row {
                    pin: "7".to_owned(),
                    kind: Kind::Face,
                    finger_index: 0,
                    size: 6,
                    valid: true,
                    data: bytes()
                },
            ]
        );
    }

    #[test]
    fn biophoto_di_operlog_dan_tabledata() {
        let operlog = parse_operlog(&format!(
            "BIOPHOTO PIN=7\tFileName=7.jpg\tType=9\tSize=6\tContent={TMP}\n"
        ));
        assert_eq!(operlog.rows.len(), 1);
        assert_eq!(operlog.rows[0].kind, Kind::FacePhoto);
        assert_eq!(operlog.rows[0].data, bytes());

        let table = parse_tabledata(
            "biophoto",
            &format!("biophoto pin=7\tfilename=7.jpg\ttype=9\tsize=6\tcontent={TMP}\n"),
        );
        assert_eq!(table.rows, operlog.rows);
    }

    #[test]
    fn templatev10_di_tabledata() {
        let parsed = parse_tabledata(
            "templatev10",
            &format!("templatev10 pin=7\tfingerid=3\tsize=6\tvalid=1\ttemplate={TMP}\r\n"),
        );

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].kind, Kind::Fingerprint);
        assert_eq!(parsed.rows[0].finger_index, 3);
        assert_eq!(parsed.rows[0].data, bytes());

        // Tabel lain tidak diproses.
        assert_eq!(parse_tabledata("user", "user pin=1"), Parsed::default());
        assert_eq!(
            parse_tabledata("extuser", "extuser pin=1"),
            Parsed::default()
        );
    }

    #[test]
    fn spasi_menggantikan_tanda_plus_seperti_kode_lama() {
        // base64 "+/8=" (bytes FB FF) dikirim sebagai " /8=".
        let parsed = parse_operlog("FP PIN=1\tFID=0\tSize=2\tValid=1\tTMP= /8=\n");

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].data, vec![0xFB, 0xFF]);
    }

    #[test]
    fn spasi_di_awal_dan_akhir_nilai_juga_tanda_plus() {
        // bytes FB EF BE = "++++"; dikirim sebagai empat spasi. Pemangkasan akan menghapusnya.
        let parsed = parse_operlog(
            "FP PIN=1	FID=0	Size=3	Valid=1	TMP=    
",
        );

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].data, vec![0xFB, 0xEF, 0xBE]);
    }

    #[test]
    fn valid_nol_ditandai_tidak_valid_dan_size_hilang_memakai_panjang_data() {
        let parsed = parse_operlog(&format!("FP PIN=1\tFID=0\tValid=0\tTMP={TMP}\n"));

        assert!(!parsed.rows[0].valid);
        assert_eq!(parsed.rows[0].size, 6);
    }

    #[test]
    fn baris_rusak_dilewati_dan_dihitung() {
        let body = format!(
            "FP PIN=\tFID=0\tSize=6\tValid=1\tTMP={TMP}\n\
             FP PIN=1\tSize=6\tValid=1\tTMP={TMP}\n\
             FP PIN=1\tFID=999\tSize=6\tValid=1\tTMP={TMP}\n\
             FP PIN=1\tFID=0\tSize=6\tValid=1\tTMP=bukan-base64!!\n\
             FP PIN=1\tFID=0\tSize=6\tValid=1\tTMP=\n\
             FP PIN=1\tFID=0\tSize=6\tValid=1\n\
             FP PIN=1\tFID=0\tSize=6\tValid=1\tTMP={TMP}\n"
        );

        let parsed = parse_operlog(&body);

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.skipped, 6);
    }

    #[test]
    fn template_terlalu_besar_dan_pin_terlalu_panjang_dilewati() {
        let big = STANDARD.encode(vec![1u8; MAX_TEMPLATE_BYTES + 1]);
        let ok = STANDARD.encode(vec![1u8; MAX_TEMPLATE_BYTES]);
        let body = format!(
            "FP PIN=1\tFID=0\tSize=1\tValid=1\tTMP={big}\n\
             FP PIN=1\tFID=1\tSize=1\tValid=1\tTMP={ok}\n\
             FP PIN={}\tFID=0\tSize=1\tValid=1\tTMP={TMP}\n",
            "9".repeat(33)
        );

        let parsed = parse_operlog(&body);

        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].data.len(), MAX_TEMPLATE_BYTES);
        assert_eq!(parsed.skipped, 2);
    }

    #[test]
    fn debug_tidak_memuat_isi_template() {
        let parsed = parse_operlog(&format!("FP PIN=1\tFID=0\tSize=6\tValid=1\tTMP={TMP}\n"));

        let text = format!("{:?}", parsed.rows[0]);

        assert!(text.contains("data_len"));
        assert!(!text.contains(TMP));
        assert!(!text.contains("[1, 2, 3"));
    }
}
