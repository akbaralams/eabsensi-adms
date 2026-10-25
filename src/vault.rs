//! Enkripsi template biometrik (keputusan D1): AES-256-GCM dengan kunci dari environment.
//!
//! - Nonce 12 byte acak dari CSPRNG sistem, baru untuk setiap enkripsi.
//! - AAD = `employee_id|kind|finger_index|algorithm_version`, sehingga ciphertext tidak bisa
//!   dipindahkan ke baris, pegawai, atau jenis lain tanpa ketahuan.
//! - Tiap baris menyimpan `key_id`; kunci lama tetap bisa mendekripsi setelah rotasi, dan baris
//!   ditulis ulang dengan kunci terkini saat berikutnya disimpan.
//! - Hanya ADMS yang memegang kunci. Isi template, kunci, dan galat dekripsi tidak pernah ikut
//!   ke pesan galat atau log.

use std::collections::HashMap;
use std::fmt;

use aes_gcm::aead::{Aead, Generate, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use zeroize::Zeroizing;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const MAX_KEY_ID_CHARS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VaultError {
    #[error("id kunci harus 1-16 karakter huruf, angka, '_' atau '-'")]
    InvalidKeyId,
    #[error("id kunci dipakai dua kali")]
    DuplicateKeyId,
    #[error("kunci bukan base64 yang berisi tepat 32 byte")]
    InvalidKey,
    #[error("kunci dengan id tersebut tidak dikonfigurasi")]
    UnknownKey,
    #[error("enkripsi gagal")]
    Encrypt,
    /// Sengaja tanpa rincian: kunci salah, AAD berbeda, dan data rusak tidak dibedakan.
    #[error("dekripsi gagal")]
    Decrypt,
}

/// Data yang diikat ke ciphertext. Harus sama saat enkripsi dan dekripsi.
#[derive(Debug, Clone, Copy)]
pub struct Aad<'a> {
    pub employee_id: i64,
    pub kind: &'a str,
    pub finger_index: i16,
    pub algorithm_version: &'a str,
}

impl Aad<'_> {
    fn bytes(&self) -> Vec<u8> {
        format!(
            "{}|{}|{}|{}",
            self.employee_id, self.kind, self.finger_index, self.algorithm_version
        )
        .into_bytes()
    }
}

/// Hasil enkripsi: ketiganya disimpan di `biometric_templates`.
#[derive(Clone, PartialEq, Eq)]
pub struct Sealed {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub key_id: String,
}

impl fmt::Debug for Sealed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sealed")
            .field("ciphertext_len", &self.ciphertext.len())
            .field("key_id", &self.key_id)
            .finish()
    }
}

pub struct Vault {
    current: String,
    keys: HashMap<String, Aes256Gcm>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ids: Vec<_> = self.keys.keys().collect();
        ids.sort();
        f.debug_struct("Vault")
            .field("current", &self.current)
            .field("key_ids", &ids)
            .finish()
    }
}

/// Mengurai kunci base64 (standar) 32 byte. Hasilnya dihapus dari memori saat dilepas.
pub fn parse_key(encoded: &str) -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError> {
    let decoded = Zeroizing::new(
        STANDARD
            .decode(encoded.trim())
            .map_err(|_| VaultError::InvalidKey)?,
    );
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    if decoded.len() != KEY_LEN {
        return Err(VaultError::InvalidKey);
    }
    key.copy_from_slice(&decoded);
    Ok(key)
}

pub fn valid_key_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_KEY_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Vault {
    /// `current` dipakai untuk enkripsi baru; `old` hanya untuk mendekripsi baris lama.
    pub fn new(
        current: (&str, &[u8; KEY_LEN]),
        old: &[(String, Zeroizing<[u8; KEY_LEN]>)],
    ) -> Result<Self, VaultError> {
        let mut keys = HashMap::new();
        let mut add = |id: &str, key: &[u8; KEY_LEN]| -> Result<(), VaultError> {
            if !valid_key_id(id) {
                return Err(VaultError::InvalidKeyId);
            }
            let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| VaultError::InvalidKey)?;
            if keys.insert(id.to_owned(), cipher).is_some() {
                return Err(VaultError::DuplicateKeyId);
            }
            Ok(())
        };

        add(current.0, current.1)?;
        for (id, key) in old {
            add(id, key)?;
        }

        Ok(Self {
            current: current.0.to_owned(),
            keys,
        })
    }

    pub fn current_key_id(&self) -> &str {
        &self.current
    }

    pub fn encrypt(&self, aad: &Aad<'_>, plaintext: &[u8]) -> Result<Sealed, VaultError> {
        let cipher = self.keys.get(&self.current).ok_or(VaultError::UnknownKey)?;
        let nonce = Nonce::generate();
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: &aad.bytes(),
                },
            )
            .map_err(|_| VaultError::Encrypt)?;

        Ok(Sealed {
            ciphertext,
            nonce: nonce.to_vec(),
            key_id: self.current.clone(),
        })
    }

    pub fn decrypt(
        &self,
        aad: &Aad<'_>,
        ciphertext: &[u8],
        nonce: &[u8],
        key_id: &str,
    ) -> Result<Vec<u8>, VaultError> {
        let cipher = self.keys.get(key_id).ok_or(VaultError::UnknownKey)?;
        let nonce = Nonce::try_from(nonce).map_err(|_| VaultError::Decrypt)?;
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad: &aad.bytes(),
                },
            )
            .map_err(|_| VaultError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: [u8; KEY_LEN] = [7; KEY_LEN];
    const KEY_B: [u8; KEY_LEN] = [9; KEY_LEN];

    fn aad() -> Aad<'static> {
        Aad {
            employee_id: 42,
            kind: "fingerprint",
            finger_index: 3,
            algorithm_version: "10",
        }
    }

    fn vault() -> Vault {
        Vault::new(("k1", &KEY_A), &[]).unwrap()
    }

    #[test]
    fn bolak_balik() {
        let vault = vault();
        let sealed = vault.encrypt(&aad(), b"template-rahasia").unwrap();

        assert_eq!(sealed.key_id, "k1");
        assert_eq!(sealed.nonce.len(), NONCE_LEN);
        assert_eq!(
            sealed.ciphertext.len(),
            b"template-rahasia".len() + 16,
            "tag 16 byte"
        );
        assert!(
            !sealed
                .ciphertext
                .windows(b"rahasia".len())
                .any(|w| w == b"rahasia"),
            "ciphertext tidak boleh memuat plaintext"
        );

        let plain = vault
            .decrypt(&aad(), &sealed.ciphertext, &sealed.nonce, &sealed.key_id)
            .unwrap();
        assert_eq!(plain, b"template-rahasia");
    }

    #[test]
    fn nonce_baru_untuk_setiap_enkripsi() {
        let vault = vault();
        let a = vault.encrypt(&aad(), b"sama").unwrap();
        let b = vault.encrypt(&aad(), b"sama").unwrap();

        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn aad_yang_berbeda_ditolak_di_setiap_bidang() {
        let vault = vault();
        let sealed = vault.encrypt(&aad(), b"data").unwrap();
        let open = |aad: Aad<'_>| vault.decrypt(&aad, &sealed.ciphertext, &sealed.nonce, "k1");

        assert!(open(aad()).is_ok());
        assert_eq!(
            open(Aad {
                employee_id: 43,
                ..aad()
            }),
            Err(VaultError::Decrypt)
        );
        assert_eq!(
            open(Aad {
                kind: "face",
                ..aad()
            }),
            Err(VaultError::Decrypt)
        );
        assert_eq!(
            open(Aad {
                finger_index: 4,
                ..aad()
            }),
            Err(VaultError::Decrypt)
        );
        assert_eq!(
            open(Aad {
                algorithm_version: "12",
                ..aad()
            }),
            Err(VaultError::Decrypt)
        );
    }

    #[test]
    fn ciphertext_atau_nonce_yang_diubah_ditolak() {
        let vault = vault();
        let sealed = vault.encrypt(&aad(), b"data").unwrap();

        let mut tampered = sealed.ciphertext.clone();
        tampered[0] ^= 1;
        assert_eq!(
            vault.decrypt(&aad(), &tampered, &sealed.nonce, "k1"),
            Err(VaultError::Decrypt)
        );

        let mut nonce = sealed.nonce.clone();
        nonce[0] ^= 1;
        assert_eq!(
            vault.decrypt(&aad(), &sealed.ciphertext, &nonce, "k1"),
            Err(VaultError::Decrypt)
        );
        assert_eq!(
            vault.decrypt(&aad(), &sealed.ciphertext, &sealed.nonce[..5], "k1"),
            Err(VaultError::Decrypt),
            "nonce dengan panjang salah"
        );
    }

    #[test]
    fn kunci_lama_tetap_bisa_membuka_setelah_rotasi() {
        let old_vault = Vault::new(("k1", &KEY_A), &[]).unwrap();
        let sealed_old = old_vault.encrypt(&aad(), b"lama").unwrap();

        // Rotasi: kunci terkini k2, k1 dipertahankan hanya untuk membaca.
        let rotated =
            Vault::new(("k2", &KEY_B), &[("k1".to_owned(), Zeroizing::new(KEY_A))]).unwrap();

        let plain = rotated
            .decrypt(&aad(), &sealed_old.ciphertext, &sealed_old.nonce, "k1")
            .unwrap();
        assert_eq!(plain, b"lama");
        assert_eq!(rotated.current_key_id(), "k2");
        assert_eq!(rotated.encrypt(&aad(), b"baru").unwrap().key_id, "k2");
    }

    #[test]
    fn kunci_yang_tidak_dikonfigurasi_atau_salah_ditolak() {
        let sealed = vault().encrypt(&aad(), b"data").unwrap();

        // k1 tidak ada di vault ini.
        let other = Vault::new(("k2", &KEY_B), &[]).unwrap();
        assert_eq!(
            other.decrypt(&aad(), &sealed.ciphertext, &sealed.nonce, "k1"),
            Err(VaultError::UnknownKey)
        );
        // id sama, kunci berbeda.
        let wrong = Vault::new(("k1", &KEY_B), &[]).unwrap();
        assert_eq!(
            wrong.decrypt(&aad(), &sealed.ciphertext, &sealed.nonce, "k1"),
            Err(VaultError::Decrypt)
        );
    }

    #[test]
    fn id_kunci_divalidasi_dan_tidak_boleh_ganda() {
        assert!(valid_key_id("k1"));
        assert!(valid_key_id("kunci-2026_a"));
        assert!(!valid_key_id(""));
        assert!(!valid_key_id("spasi salah"));
        assert!(!valid_key_id(&"k".repeat(17)));

        assert_eq!(
            Vault::new(("a b", &KEY_A), &[]).unwrap_err(),
            VaultError::InvalidKeyId
        );
        assert_eq!(
            Vault::new(("k1", &KEY_A), &[("k1".to_owned(), Zeroizing::new(KEY_B))]).unwrap_err(),
            VaultError::DuplicateKeyId
        );
    }

    #[test]
    fn parse_key_hanya_menerima_base64_32_byte() {
        let encoded = STANDARD.encode(KEY_A);
        assert_eq!(*parse_key(&encoded).unwrap(), KEY_A);
        assert_eq!(*parse_key(&format!("  {encoded}\n")).unwrap(), KEY_A);

        assert_eq!(parse_key("").unwrap_err(), VaultError::InvalidKey);
        assert_eq!(
            parse_key("bukan base64!!").unwrap_err(),
            VaultError::InvalidKey
        );
        assert_eq!(
            parse_key(&STANDARD.encode([1u8; 16])).unwrap_err(),
            VaultError::InvalidKey
        );
        assert_eq!(
            parse_key(&STANDARD.encode([1u8; 33])).unwrap_err(),
            VaultError::InvalidKey
        );
    }

    #[test]
    fn debug_tidak_membocorkan_kunci_atau_isi() {
        let vault = vault();
        let sealed = vault.encrypt(&aad(), b"isi-sangat-rahasia").unwrap();

        let text = format!("{vault:?} {sealed:?}");

        assert!(text.contains("k1"));
        assert!(!text.contains("rahasia"));
        assert!(!text.contains("[7, 7"), "kunci tidak boleh tercetak");
        assert!(!text.contains(&format!("{:?}", sealed.nonce)));
    }
}
