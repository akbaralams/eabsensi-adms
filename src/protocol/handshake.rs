//! Balasan handshake `GET /iclock/cdata` (opsi per mesin). Setiap baris berakhir LF.

use super::zk_time;

/// Stamp terakhir yang dikirim mesin; kosong dikirim sebagai `None`.
#[derive(Debug, Default, Clone)]
pub struct Stamps {
    pub attlog: Option<String>,
    pub operlog: Option<String>,
    pub photo: Option<String>,
    pub biodata: Option<String>,
}

/// Bahan balasan handshake sebuah mesin.
#[derive(Debug, Clone)]
pub struct HandshakeDevice {
    pub serial: String,
    pub stamps: Stamps,
    pub poll_delay_seconds: i16,
    pub error_delay_seconds: i16,
    /// Selisih zona mesin terhadap UTC, menit.
    pub offset_minutes: i32,
}

/// Tiga bentuk balasan opsi yang dikenal kode lama.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Firmware lama: `Stamp`/`OpStamp`/`PhotoStamp`, `TransFlag=111111111111`.
    Basic,
    /// Mesin absensi "att" / ADMS: `ATTLOGStamp` dan kawan-kawan.
    Att,
    /// Firmware dengan versi push tercatat: tambah `PushProtVer=2.3.0`.
    Push23,
}

/// Bentuk push 3.x: `pushver` diawali `3.` dan `PushOptionsFlag=1`.
pub fn is_push3(pushver: Option<&str>, push_options_flag: Option<&str>) -> bool {
    pushver.is_some_and(|version| version.starts_with("3.")) && push_options_flag == Some("1")
}

/// Balasan untuk firmware push 3.x (tanpa LF akhir, persis kode lama).
pub fn render_push3(pushver: &str) -> String {
    format!("OK\nPushProtVer={pushver}\nPushOptionsFlag=1")
}

/// Memilih bentuk balasan seperti percabangan `iconnect` lama.
///
/// - Belum diinisialisasi (`is_new`): `att` bila request membawa `DeviceType=att|0`, selain itu dasar.
/// - Sudah, dengan versi push tercatat: `att` bila tipe mesin `att`, selain itu `Push23`.
/// - Sudah, tanpa versi push: dasar.
pub fn select_format(
    initialized: bool,
    push_version: Option<&str>,
    stored_is_att: bool,
    requested_device_type: Option<&str>,
) -> Format {
    if !initialized {
        return if matches!(requested_device_type, Some("att" | "0")) {
            Format::Att
        } else {
            Format::Basic
        };
    }
    match push_version {
        Some(_) if stored_is_att => Format::Att,
        Some(_) => Format::Push23,
        None => Format::Basic,
    }
}

pub fn render(format: Format, device: &HandshakeDevice) -> String {
    let stamp = |value: &Option<String>| value.clone().unwrap_or_else(|| "None".to_owned());
    let timezone = zk_time::hours_label(device.offset_minutes);
    let mut out = format!("GET OPTION FROM:{}\n", device.serial);

    match format {
        Format::Att => {
            out.push_str(&format!("ATTLOGStamp={}\n", stamp(&device.stamps.attlog)));
            out.push_str(&format!("OPERLOGStamp={}\n", stamp(&device.stamps.operlog)));
            out.push_str(&format!("BIODATAStamp={}\n", stamp(&device.stamps.biodata)));
            out.push_str(&format!("ATTPHOTOStamp={}\n", stamp(&device.stamps.photo)));
        }
        Format::Basic | Format::Push23 => {
            out.push_str(&format!("Stamp={}\n", stamp(&device.stamps.attlog)));
            out.push_str(&format!("OpStamp={}\n", stamp(&device.stamps.operlog)));
            out.push_str(&format!("PhotoStamp={}\n", stamp(&device.stamps.photo)));
        }
    }

    out.push_str(&format!("ErrorDelay={}\n", device.error_delay_seconds));
    out.push_str(&format!("Delay={}\n", device.poll_delay_seconds));

    if format == Format::Push23 {
        out.push_str("PushProtVer=2.3.0\n");
        out.push_str("ServerVer=2.3.0\n");
        out.push_str("EncryptFlag=1000000000\n");
    }

    out.push_str("TransTimes=00:00;14:05\n");
    out.push_str("TransInterval=1\n");
    out.push_str(match format {
        Format::Basic => "TransFlag=111111111111\n",
        Format::Att => "TransFlag=111110101101\n",
        Format::Push23 => {
            "TransFlag=TransData AttLog\tOpLog\tAttPhoto\tEnrollFP\tEnrollUser\tFPImag\tChgUser\tChgFP\tFACE\tUserPic\tFVEIN\tBioPhoto\n"
        }
    });
    out.push_str(&format!("TimeZone={timezone}\n"));
    out.push_str("Realtime=1\n");
    out.push_str("Encrypt=0\n");

    if format == Format::Att {
        out.push_str("ServerVer=adms2.0\n");
        out.push_str(
            "PushOptions=RegDeviceType,FingerFunOn,FaceFunOn,FPVersion,FaceVersion,NetworkType\n",
        );
        out.push_str("MultiBioDataSupport=0:1:1:0:0:0:0:0:1:1\n");
        out.push_str("MultiBioPhotoSupport=0:0:0:0:0:0:0:0:0:1\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push3_dikenali_hanya_bila_versi_3_dan_flag_1() {
        assert!(is_push3(Some("3.0.1"), Some("1")));
        assert!(!is_push3(Some("2.1.44"), Some("1")));
        assert!(!is_push3(Some("3.0.1"), Some("0")));
        assert!(!is_push3(Some("3.0.1"), None));
        assert!(!is_push3(None, Some("1")));
    }

    #[test]
    fn balasan_push3_persis_tanpa_lf_akhir() {
        assert_eq!(
            render_push3("3.0.1"),
            "OK\nPushProtVer=3.0.1\nPushOptionsFlag=1"
        );
    }

    #[test]
    fn pemilihan_bentuk_mengikuti_percabangan_lama() {
        // Mesin baru: bentuk ditentukan parameter DeviceType di request.
        assert_eq!(select_format(false, None, false, Some("att")), Format::Att);
        assert_eq!(select_format(false, None, false, Some("0")), Format::Att);
        assert_eq!(
            select_format(false, None, false, Some("acc")),
            Format::Basic
        );
        assert_eq!(select_format(false, Some("2.1"), true, None), Format::Basic);
        // Mesin lama dengan versi push: ditentukan tipe tersimpan.
        assert_eq!(select_format(true, Some("2.1"), true, None), Format::Att);
        assert_eq!(
            select_format(true, Some("2.1"), false, None),
            Format::Push23
        );
        // Mesin lama tanpa versi push: dasar, tipe diabaikan.
        assert_eq!(select_format(true, None, true, Some("att")), Format::Basic);
    }
}
