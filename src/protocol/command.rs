//! Perintah ke mesin (`C:<id>:<perintah>`) dan hasil perintah dari mesin (`devicecmd`).

use super::kv;

/// Jenis perintah yang boleh diantrekan (keputusan D8). Nilai `as_db` sama dengan
/// `DeviceCommandType` di eabsensi-v2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Info,
    Check,
    Reboot,
    SetTime,
    AddUser,
    AddFingerprint,
}

impl CommandKind {
    pub fn from_db(value: &str) -> Option<Self> {
        Some(match value {
            "info" => Self::Info,
            "check" => Self::Check,
            "reboot" => Self::Reboot,
            "set_time" => Self::SetTime,
            "add_user" => Self::AddUser,
            "add_fingerprint" => Self::AddFingerprint,
            _ => return None,
        })
    }

    pub fn as_db(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Check => "check",
            Self::Reboot => "reboot",
            Self::SetTime => "set_time",
            Self::AddUser => "add_user",
            Self::AddFingerprint => "add_fingerprint",
        }
    }

    /// Kelas prioritas antrean: 1 sistem, 2 pengaturan, 3 user, 4 jari/wajah.
    pub fn priority(self) -> i16 {
        match self {
            Self::Info | Self::Check | Self::Reboot => 1,
            Self::SetTime => 2,
            Self::AddUser => 3,
            Self::AddFingerprint => 4,
        }
    }
}

/// Jumlah perintah yang dikirim dalam satu `getrequest`, per kelas prioritas (kode lama).
pub fn batch_limit(priority: i16) -> i64 {
    match priority {
        1 => 1,
        2 => 10,
        3 => 50,
        _ => 5,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct User<'a> {
    pub pin: &'a str,
    pub name: &'a str,
}

/// Template sidik jari yang sudah didekripsi, siap dikirim ke mesin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint<'a> {
    pub finger_index: i16,
    /// `Size` asli dari mesin (byte).
    pub size: i32,
    /// Template dalam base64 standar.
    pub template: &'a str,
}

/// Bahan merakit satu perintah.
#[derive(Debug, Clone, Copy)]
pub struct Render<'a> {
    pub id: i64,
    pub kind: CommandKind,
    /// Selisih zona mesin terhadap UTC, menit (untuk `MachineTZ`).
    pub offset_minutes: i32,
    /// Detik ZK untuk `DateTime` (`zk_time::encode` dari waktu UTC sekarang).
    pub zk_now: i64,
    pub user: Option<User<'a>>,
    pub fingerprint: Option<Fingerprint<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    #[error("pegawai untuk perintah ini tidak ditemukan")]
    MissingUser,
    #[error("PIN mengandung karakter yang tidak boleh dikirim ke mesin")]
    InvalidPin,
    #[error("jenis perintah ini belum didukung server")]
    Unsupported,
    #[error("data biometrik untuk perintah ini tidak tersedia")]
    MissingBiometric,
}

/// Merakit baris perintah, masing-masing berakhir LF. `set_time` menghasilkan dua baris dengan
/// `ID` yang sama (zona lalu jam), persis dua perintah terpisah di kode lama; mesin akan menjawab
/// keduanya dan jawaban pertama yang menentukan hasil.
pub fn render(input: &Render<'_>) -> Result<String, RenderError> {
    let id = input.id;
    match input.kind {
        CommandKind::Info => Ok(format!("C:{id}:INFO\n")),
        CommandKind::Check => Ok(format!("C:{id}:CHECK\n")),
        CommandKind::Reboot => Ok(format!("C:{id}:REBOOT\n")),
        CommandKind::SetTime => Ok(format!(
            "C:{id}:SET OPTIONS MachineTZ={}\nC:{id}:SET OPTIONS DateTime={}\n",
            super::zk_time::hhmm(input.offset_minutes),
            input.zk_now
        )),
        CommandKind::AddUser => {
            let user = input.user.ok_or(RenderError::MissingUser)?;
            if !is_valid_pin(user.pin) {
                return Err(RenderError::InvalidPin);
            }
            Ok(format!(
                "C:{id}:DATA USER PIN={}\tName={}\tPri=0\n",
                user.pin,
                sanitize_name(user.name)
            ))
        }
        CommandKind::AddFingerprint => {
            let user = input.user.ok_or(RenderError::MissingUser)?;
            if !is_valid_pin(user.pin) {
                return Err(RenderError::InvalidPin);
            }
            let fingerprint = input.fingerprint.ok_or(RenderError::MissingBiometric)?;
            Ok(format!(
                "C:{id}:DATA FP PIN={}\tFID={}\tSize={}\tValid=1\tTMP={}\n",
                user.pin, fingerprint.finger_index, fingerprint.size, fingerprint.template
            ))
        }
    }
}

/// PIN yang aman dikirim: huruf, angka, `_` dan `-` (pola `^[\w-]+$` di kode lama).
pub fn is_valid_pin(pin: &str) -> bool {
    !pin.is_empty()
        && pin
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Nama dikirim di baris yang dipisah tab dan LF, jadi karakter kontrol diganti spasi.
fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Satu jawaban mesin atas sebuah perintah.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub id: i64,
    /// `None` bila `Return` tidak dapat dibaca; diperlakukan sebagai gagal.
    pub return_code: Option<i32>,
    pub cmd: String,
}

impl CommandResult {
    pub fn is_success(&self) -> bool {
        self.return_code == Some(0)
    }
}

/// Isi `POST /iclock/devicecmd`.
#[derive(Debug, PartialEq, Eq)]
pub enum DeviceCmdBody {
    /// Badan `CMD=INFO`: jawaban beserta seluruh pasangan info mesin.
    Info {
        result: CommandResult,
        pairs: Vec<(String, String)>,
    },
    /// Satu atau lebih baris `ID=..&Return=..&CMD=..`.
    Results(Vec<CommandResult>),
}

pub fn parse_devicecmd(body: &str) -> DeviceCmdBody {
    if body.contains("CMD=INFO") {
        let pairs = kv::pairs(body, false);
        if let Some(result) = result_from_pairs(&pairs) {
            return DeviceCmdBody::Info { result, pairs };
        }
    }

    let results = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| result_from_pairs(&kv::pairs(line, false)))
        .collect();
    DeviceCmdBody::Results(results)
}

fn result_from_pairs(pairs: &[(String, String)]) -> Option<CommandResult> {
    let id = kv::last(pairs, "ID")?.trim().parse::<i64>().ok()?;
    // Kode lama membuang semua karakter selain angka dan minus dari `Return`.
    let return_code = kv::last(pairs, "Return").and_then(|raw| {
        raw.chars()
            .filter(|c| c.is_ascii_digit() || *c == '-')
            .collect::<String>()
            .parse::<i32>()
            .ok()
    });
    let cmd = kv::last(pairs, "CMD").unwrap_or_default().to_owned();
    Some(CommandResult {
        id,
        return_code,
        cmd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(kind: CommandKind) -> Render<'static> {
        Render {
            id: 90470,
            kind,
            offset_minutes: 420,
            zk_now: 785_149_992,
            user: None,
            fingerprint: None,
        }
    }

    #[test]
    fn jenis_perintah_bolak_balik_dengan_nilai_database() {
        for kind in [
            CommandKind::Info,
            CommandKind::Check,
            CommandKind::Reboot,
            CommandKind::SetTime,
            CommandKind::AddUser,
            CommandKind::AddFingerprint,
        ] {
            assert_eq!(CommandKind::from_db(kind.as_db()), Some(kind));
        }
        assert_eq!(CommandKind::from_db("clear_log"), None);
    }

    #[test]
    fn prioritas_dan_batas_sama_dengan_kelas_lama() {
        assert_eq!(CommandKind::Info.priority(), 1);
        assert_eq!(CommandKind::Reboot.priority(), 1);
        assert_eq!(CommandKind::SetTime.priority(), 2);
        assert_eq!(CommandKind::AddUser.priority(), 3);
        assert_eq!(CommandKind::AddFingerprint.priority(), 4);
        assert_eq!(
            [1, 2, 3, 4].map(batch_limit),
            [1, 10, 50, 5],
            "batas per poll: sistem 1, pengaturan 10, user 50, jari 5"
        );
    }

    #[test]
    fn perintah_sederhana_cocok_dengan_log_produksi() {
        // Log produksi lama: "C:90470:CHECK" lalu LF.
        assert_eq!(
            render(&base(CommandKind::Check)).unwrap(),
            "C:90470:CHECK\n"
        );
        assert_eq!(render(&base(CommandKind::Info)).unwrap(), "C:90470:INFO\n");
        assert_eq!(
            render(&base(CommandKind::Reboot)).unwrap(),
            "C:90470:REBOOT\n"
        );
    }

    #[test]
    fn set_time_dua_baris_dengan_id_sama() {
        assert_eq!(
            render(&base(CommandKind::SetTime)).unwrap(),
            "C:90470:SET OPTIONS MachineTZ=+0700\nC:90470:SET OPTIONS DateTime=785149992\n"
        );
    }

    #[test]
    fn add_user_memakai_format_lama() {
        let mut input = base(CommandKind::AddUser);
        input.user = Some(User {
            pin: "12345",
            name: "Budi Santoso",
        });

        assert_eq!(
            render(&input).unwrap(),
            "C:90470:DATA USER PIN=12345\tName=Budi Santoso\tPri=0\n"
        );
    }

    #[test]
    fn add_user_membersihkan_nama_dan_menolak_pin_berbahaya() {
        let mut input = base(CommandKind::AddUser);
        input.user = Some(User {
            pin: "7",
            name: "A\tB\nC\r\n  D",
        });
        assert_eq!(
            render(&input).unwrap(),
            "C:90470:DATA USER PIN=7\tName=A B C D\tPri=0\n"
        );

        for pin in ["", "1 2", "1\t2", "1\n2", "a=b", "x&y"] {
            input.user = Some(User { pin, name: "N" });
            assert_eq!(render(&input), Err(RenderError::InvalidPin), "pin {pin:?}");
        }

        input.user = None;
        assert_eq!(render(&input), Err(RenderError::MissingUser));
    }

    #[test]
    fn add_fingerprint_memakai_format_lama() {
        let mut input = base(CommandKind::AddFingerprint);
        input.user = Some(User {
            pin: "12345",
            name: "Budi",
        });
        input.fingerprint = Some(Fingerprint {
            finger_index: 3,
            size: 6,
            template: "AQIDBAUG",
        });

        assert_eq!(
            render(&input).unwrap(),
            "C:90470:DATA FP PIN=12345\tFID=3\tSize=6\tValid=1\tTMP=AQIDBAUG\n"
        );
    }

    #[test]
    fn add_fingerprint_menolak_data_yang_tidak_lengkap_atau_pin_berbahaya() {
        let mut input = base(CommandKind::AddFingerprint);
        assert_eq!(render(&input), Err(RenderError::MissingUser));

        input.user = Some(User {
            pin: "1",
            name: "N",
        });
        assert_eq!(render(&input), Err(RenderError::MissingBiometric));

        input.user = Some(User {
            pin: "1\t2",
            name: "N",
        });
        input.fingerprint = Some(Fingerprint {
            finger_index: 0,
            size: 1,
            template: "AA==",
        });
        assert_eq!(render(&input), Err(RenderError::InvalidPin));
    }

    #[test]
    fn hasil_perintah_biasa_dari_log_produksi() {
        // 28 byte di log produksi = "ID=90470&Return=0&CMD=CHECK" + LF.
        let body = "ID=90470&Return=0&CMD=CHECK\n";
        assert_eq!(body.len(), 28);

        assert_eq!(
            parse_devicecmd(body),
            DeviceCmdBody::Results(vec![CommandResult {
                id: 90470,
                return_code: Some(0),
                cmd: "CHECK".to_owned()
            }])
        );
    }

    #[test]
    fn beberapa_hasil_dalam_satu_badan() {
        let body = "ID=1&Return=0&CMD=SET OPTIONS\r\nID=1&Return=0&CMD=SET OPTIONS\nID=2&Return=-1001&CMD=DATA\n\n";

        let DeviceCmdBody::Results(results) = parse_devicecmd(body) else {
            panic!("seharusnya Results");
        };

        assert_eq!(results.len(), 3);
        assert_eq!(results[2].id, 2);
        assert_eq!(results[2].return_code, Some(-1001));
        assert!(results[0].is_success());
        assert!(!results[2].is_success());
    }

    #[test]
    fn badan_info_membawa_semua_pasangan() {
        let body = "ID=5\nReturn=0\nCMD=INFO\nPushVersion=2.1.44\nUserCount=3\n";

        let DeviceCmdBody::Info { result, pairs } = parse_devicecmd(body) else {
            panic!("seharusnya Info");
        };

        assert_eq!(result.id, 5);
        assert!(result.is_success());
        assert_eq!(kv::last(&pairs, "PushVersion"), Some("2.1.44"));
    }

    #[test]
    fn return_tidak_terbaca_dianggap_gagal_dan_baris_tanpa_id_diabaikan() {
        let DeviceCmdBody::Results(results) =
            parse_devicecmd("ID=3&Return=abc&CMD=X\nReturn=0&CMD=Y\nID=zz&Return=0\n")
        else {
            panic!("seharusnya Results");
        };

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].return_code, None);
        assert!(!results[0].is_success());
    }

    #[test]
    fn badan_kosong_tidak_menghasilkan_apa_apa() {
        assert_eq!(parse_devicecmd(""), DeviceCmdBody::Results(vec![]));
    }
}
