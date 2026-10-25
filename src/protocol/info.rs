//! Informasi perangkat yang dilaporkan mesin (`INFO` di getrequest, badan `CMD=INFO`, `options`).

use super::kv;

/// Kolom `devices` yang diisi dari laporan mesin. `None` berarti mesin tidak melaporkannya.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub firmware_version: Option<String>,
    pub push_version: Option<String>,
    pub ip_address: Option<String>,
    pub model: Option<String>,
    pub platform: Option<String>,
    pub user_count: Option<i32>,
    pub fingerprint_count: Option<i32>,
    pub face_count: Option<i32>,
    pub log_count: Option<i32>,
}

impl DeviceInfo {
    /// Dari parameter `INFO=<firmware>,<jumlah user>,<jumlah jari>,<jumlah log>,<ip>,...`
    /// di `getrequest`. Kurang dari lima kolom tidak menjadi galat (kode lama menjadi 500).
    pub fn from_info_param(value: &str) -> Self {
        let mut fields = value.split(',').map(str::trim);
        let mut next = || fields.next().filter(|field| !field.is_empty());

        Self {
            firmware_version: next().map(str::to_owned),
            user_count: next().and_then(parse_count),
            fingerprint_count: next().and_then(parse_count),
            log_count: next().and_then(parse_count),
            ip_address: next().map(str::to_owned),
            ..Self::default()
        }
    }

    /// Dari pasangan `kunci=nilai` badan `CMD=INFO`, `options`, dan `registry`.
    pub fn from_pairs(pairs: &[(String, String)]) -> Self {
        let text = |key: &str| {
            kv::last(pairs, key)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let count = |key: &str| kv::last(pairs, key).and_then(parse_count);

        Self {
            firmware_version: None,
            push_version: text("PushVersion"),
            ip_address: text("IPAddress"),
            model: text("~DeviceName"),
            platform: text("~Platform"),
            user_count: count("UserCount"),
            fingerprint_count: count("FPCount"),
            face_count: count("FaceCount"),
            log_count: count("TransactionCount"),
        }
    }
}

fn parse_count(raw: &str) -> Option<i32> {
    raw.trim().parse::<i32>().ok().filter(|n| *n >= 0)
}

/// Membatasi isi yang disimpan ke `devices.reported_options`: jumlah dan panjang dibatasi agar
/// mesin yang rusak atau berbahaya tidak menggembungkan baris.
pub fn bounded_options(pairs: &[(String, String)]) -> serde_json::Value {
    const MAX_PAIRS: usize = 100;
    const MAX_VALUE_CHARS: usize = 255;

    let map = pairs
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "ID" | "Return" | "CMD"))
        .take(MAX_PAIRS)
        .map(|(key, value)| {
            let value: String = value.chars().take(MAX_VALUE_CHARS).collect();
            (key.clone(), serde_json::Value::String(value))
        })
        .collect();
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_param_lengkap() {
        let info = DeviceInfo::from_info_param("Ver 6.60 Apr 28 2019,12,20,345,192.168.1.50,1,2");

        assert_eq!(
            info.firmware_version.as_deref(),
            Some("Ver 6.60 Apr 28 2019")
        );
        assert_eq!(info.user_count, Some(12));
        assert_eq!(info.fingerprint_count, Some(20));
        assert_eq!(info.log_count, Some(345));
        assert_eq!(info.ip_address.as_deref(), Some("192.168.1.50"));
    }

    #[test]
    fn info_param_pendek_tidak_gagal() {
        let info = DeviceInfo::from_info_param("Ver 1.0,3");

        assert_eq!(info.firmware_version.as_deref(), Some("Ver 1.0"));
        assert_eq!(info.user_count, Some(3));
        assert_eq!(info.fingerprint_count, None);
        assert_eq!(info.ip_address, None);
        assert_eq!(DeviceInfo::from_info_param(""), DeviceInfo::default());
    }

    #[test]
    fn info_param_menolak_angka_tidak_valid_dan_negatif() {
        let info = DeviceInfo::from_info_param("v,abc,-4,9999999999,1.2.3.4");

        assert_eq!(info.user_count, None);
        assert_eq!(info.fingerprint_count, None);
        assert_eq!(info.log_count, None);
        assert_eq!(info.ip_address.as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn dari_pasangan_cmd_info() {
        let pairs = kv::pairs(
            "ID=1&Return=0&CMD=INFO\nPushVersion=2.1.44\nUserCount=7\nFPCount=9\nFaceCount=0\nTransactionCount=120\n~DeviceName=SpeedFace\n~Platform=ZMM220\nIPAddress=10.0.0.9",
            false,
        );

        let info = DeviceInfo::from_pairs(&pairs);

        assert_eq!(info.push_version.as_deref(), Some("2.1.44"));
        assert_eq!(info.user_count, Some(7));
        assert_eq!(info.fingerprint_count, Some(9));
        assert_eq!(info.face_count, Some(0));
        assert_eq!(info.log_count, Some(120));
        assert_eq!(info.model.as_deref(), Some("SpeedFace"));
        assert_eq!(info.platform.as_deref(), Some("ZMM220"));
        assert_eq!(info.ip_address.as_deref(), Some("10.0.0.9"));
    }

    #[test]
    fn opsi_yang_disimpan_dibatasi_dan_tanpa_kunci_perintah() {
        let mut pairs = vec![
            ("ID".to_owned(), "1".to_owned()),
            ("Return".to_owned(), "0".to_owned()),
            ("CMD".to_owned(), "INFO".to_owned()),
            ("Panjang".to_owned(), "x".repeat(1000)),
        ];
        pairs.extend((0..200).map(|i| (format!("K{i}"), "v".to_owned())));

        let json = bounded_options(&pairs);
        let object = json.as_object().unwrap();

        assert_eq!(object.len(), 100);
        assert!(!object.contains_key("ID"));
        assert_eq!(object["Panjang"].as_str().unwrap().chars().count(), 255);
    }
}
