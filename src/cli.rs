//! Subperintah baris perintah untuk operasional. Tanpa argumen, biner menjalankan server.

pub const USAGE: &str = "\
eabsensi-adms: server ADMS (protokol push iClock) untuk mesin absensi ZKTeco

PEMAKAIAN
  eabsensi-adms                       jalankan server (sama dengan `serve`)
  eabsensi-adms check-offline [--factor N] [--min-seconds N]
        daftar mesin aktif yang tidak terlihat lebih lama dari
        max(N x poll_delay_seconds, min-seconds); bawaan N=3, min-seconds=120.
        Status keluar: 0 semua online, 2 ada yang offline, 1 galat.
  eabsensi-adms rekey-biometrics [--from ID_KUNCI] [--dry-run]
        enkripsi ulang template ke kunci terkini (BIOMETRIC_KEY_ID) setelah rotasi.
        --from membatasi ke baris dengan key_id tertentu; --dry-run hanya memeriksa
        bahwa semua baris bisa dibuka tanpa mengubah apa pun.
        Status keluar: 0 sukses, 2 ada baris yang tidak bisa dibuka, 1 galat.
  eabsensi-adms --version | --help

Konfigurasi dari environment (lihat .env.example).";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve,
    CheckOffline { factor: u32, min_seconds: u32 },
    RekeyBiometrics { from: Option<String>, dry_run: bool },
    Help,
    Version,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Command::Serve);
    };
    let rest: Vec<String> = args.collect();

    match first.as_str() {
        "serve" if rest.is_empty() => Ok(Command::Serve),
        "--help" | "-h" | "help" => Ok(Command::Help),
        "--version" | "-V" => Ok(Command::Version),
        "check-offline" => check_offline(&rest),
        "rekey-biometrics" => rekey(&rest),
        other => Err(format!("perintah tidak dikenal: {other}")),
    }
}

fn number(flag: &str, value: Option<&String>) -> Result<u32, String> {
    value
        .ok_or_else(|| format!("{flag} butuh nilai"))?
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("{flag} harus bilangan bulat lebih dari 0"))
}

fn check_offline(args: &[String]) -> Result<Command, String> {
    let (mut factor, mut min_seconds) = (3, 120);
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--factor" => factor = number(flag, iter.next())?,
            "--min-seconds" => min_seconds = number(flag, iter.next())?,
            other => return Err(format!("opsi tidak dikenal untuk check-offline: {other}")),
        }
    }
    Ok(Command::CheckOffline {
        factor,
        min_seconds,
    })
}

fn rekey(args: &[String]) -> Result<Command, String> {
    let (mut from, mut dry_run) = (None, false);
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--dry-run" => dry_run = true,
            "--from" => {
                from = Some(
                    iter.next()
                        .ok_or_else(|| "--from butuh id kunci".to_owned())?
                        .clone(),
                );
            }
            other => {
                return Err(format!(
                    "opsi tidak dikenal untuk rekey-biometrics: {other}"
                ));
            }
        }
    }
    Ok(Command::RekeyBiometrics { from, dry_run })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn tanpa_argumen_menjalankan_server() {
        assert_eq!(parse_str(&[]), Ok(Command::Serve));
        assert_eq!(parse_str(&["serve"]), Ok(Command::Serve));
    }

    #[test]
    fn bantuan_dan_versi() {
        for flag in ["--help", "-h", "help"] {
            assert_eq!(parse_str(&[flag]), Ok(Command::Help));
        }
        assert_eq!(parse_str(&["--version"]), Ok(Command::Version));
        assert_eq!(parse_str(&["-V"]), Ok(Command::Version));
    }

    #[test]
    fn check_offline_dengan_bawaan_dan_opsi() {
        assert_eq!(
            parse_str(&["check-offline"]),
            Ok(Command::CheckOffline {
                factor: 3,
                min_seconds: 120
            })
        );
        assert_eq!(
            parse_str(&["check-offline", "--factor", "5", "--min-seconds", "30"]),
            Ok(Command::CheckOffline {
                factor: 5,
                min_seconds: 30
            })
        );
    }

    #[test]
    fn rekey_dengan_opsi() {
        assert_eq!(
            parse_str(&["rekey-biometrics"]),
            Ok(Command::RekeyBiometrics {
                from: None,
                dry_run: false
            })
        );
        assert_eq!(
            parse_str(&["rekey-biometrics", "--dry-run", "--from", "k1"]),
            Ok(Command::RekeyBiometrics {
                from: Some("k1".to_owned()),
                dry_run: true
            })
        );
    }

    #[test]
    fn masukan_salah_ditolak_dengan_pesan() {
        assert!(parse_str(&["jalan"]).unwrap_err().contains("jalan"));
        assert!(parse_str(&["serve", "x"]).is_err());
        assert!(parse_str(&["check-offline", "--factor"]).is_err());
        assert!(parse_str(&["check-offline", "--factor", "0"]).is_err());
        assert!(parse_str(&["check-offline", "--factor", "abc"]).is_err());
        assert!(parse_str(&["check-offline", "--lain", "1"]).is_err());
        assert!(parse_str(&["rekey-biometrics", "--from"]).is_err());
        assert!(parse_str(&["rekey-biometrics", "--hapus"]).is_err());
    }
}
