//! Penguraian pasangan `kunci=nilai` yang dipakai mesin di banyak tempat.

/// Memecah teks menjadi pasangan `kunci=nilai`. Pemisah pasangan: `&`, LF, CR, dan koma bila
/// `comma` benar (badan `options` dan `registry` memakai koma). Baris tanpa `=` diabaikan.
/// Nilai dipakai apa adanya (tanpa dekode persen).
pub fn pairs(text: &str, comma: bool) -> Vec<(String, String)> {
    text.split(|c| matches!(c, '&' | '\n' | '\r') || (comma && c == ','))
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

/// Nilai terakhir untuk `key` (sama dengan perilaku `parse_str` PHP: kunci ganda, yang akhir menang).
pub fn last<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memecah_baris_dan_ampersand() {
        let got = pairs(
            "ID=5&Return=0&CMD=INFO\nPushVersion=2.1.44\r\n~DeviceName=iFace",
            false,
        );
        assert_eq!(last(&got, "ID"), Some("5"));
        assert_eq!(last(&got, "CMD"), Some("INFO"));
        assert_eq!(last(&got, "PushVersion"), Some("2.1.44"));
        assert_eq!(last(&got, "~DeviceName"), Some("iFace"));
    }

    #[test]
    fn koma_hanya_memisah_bila_diminta() {
        let teks = "UserCount=3,FPCount=2";
        assert_eq!(pairs(teks, true).len(), 2);
        let satu = pairs(teks, false);
        assert_eq!(satu.len(), 1);
        assert_eq!(last(&satu, "UserCount"), Some("3,FPCount=2"));
    }

    #[test]
    fn nilai_yang_memuat_sama_dengan_dipertahankan() {
        let got = pairs("Key=a=b", false);
        assert_eq!(last(&got, "Key"), Some("a=b"));
    }

    #[test]
    fn kunci_ganda_yang_terakhir_menang() {
        let got = pairs("A=1&A=2", false);
        assert_eq!(last(&got, "A"), Some("2"));
    }

    #[test]
    fn baris_tanpa_sama_dengan_dan_kunci_kosong_diabaikan() {
        assert!(pairs("sampah\n=x\n", false).is_empty());
    }
}
