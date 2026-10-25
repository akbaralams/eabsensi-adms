//! Satu mesin simulasi: ingatan log, daftar user dan template, dan siklus koneksinya.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::NaiveDateTime;

use super::http::{Client, Response};
use super::{LatencyStats, SimError};
use crate::protocol::attlog::TIME_FORMAT;
use crate::protocol::kv;
use crate::protocol::zk_time;

/// Cara mesin memilih log yang diunggah pada tiap koneksi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resend {
    /// Mengirim seluruh isi ingatan pada setiap koneksi, seperti yang terlihat di log produksi
    /// lama (batch yang sama muncul lagi setiap koneksi).
    All,
    /// Hanya log yang waktunya (detik ZK) melebihi stamp yang dikembalikan server di handshake.
    /// Inilah hipotesis perilaku firmware bila server mengembalikan stamp apa adanya.
    FromStamp,
}

#[derive(Debug, Clone)]
pub struct MachineConfig {
    pub serial: String,
    pub user_agent: String,
    pub push_version: String,
    /// Dikirim sebagai `DeviceType=` di handshake bila diisi (mis. `att`).
    pub device_type: Option<String>,
    pub resend: Resend,
    /// Jumlah baris maksimum per request ATTLOG.
    pub max_rows_per_request: usize,
    /// Dilaporkan di INFO sebagai `~ZKFPVersion`.
    pub fp_version: Option<String>,
}

impl MachineConfig {
    pub fn new(serial: impl Into<String>) -> Self {
        Self {
            serial: serial.into(),
            user_agent: "iClock Proxy/1.09".to_owned(),
            push_version: "2.1.44".to_owned(),
            device_type: None,
            resend: Resend::All,
            max_rows_per_request: 500,
            fp_version: None,
        }
    }
}

/// Satu log absensi di ingatan mesin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub pin: String,
    /// Waktu lokal mesin.
    pub time: NaiveDateTime,
    pub status: i16,
    pub verify: i16,
}

impl Record {
    pub fn new(pin: impl Into<String>, time: NaiveDateTime) -> Self {
        Self {
            pin: pin.into(),
            time,
            status: 0,
            verify: 1,
        }
    }

    fn stamp(&self) -> i64 {
        zk_time::encode(self.time.and_utc())
    }
}

/// Opsi yang dikirim server di handshake.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub pairs: Vec<(String, String)>,
}

impl Options {
    pub fn get(&self, key: &str) -> Option<&str> {
        kv::last(&self.pairs, key)
    }

    /// Stamp ATTLOG (`Stamp` atau `ATTLOGStamp`); `None` bila `None` atau tidak ada.
    pub fn attlog_stamp(&self) -> Option<i64> {
        self.get("ATTLOGStamp")
            .or_else(|| self.get("Stamp"))
            .and_then(|value| value.parse().ok())
    }
}

#[derive(Debug, Clone)]
pub enum Handshake {
    Accepted(Options),
    /// Balasan kosong: mesin tidak dikenal, nonaktif, atau request ditolak.
    Rejected,
}

/// Perintah dari server: `C:<id>:<teks>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: i64,
    pub text: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UploadReport {
    /// Baris yang dikirim (semua request).
    pub sent: usize,
    pub requests: usize,
    /// Semua request dijawab `OK...`; bila `false` mesin menganggap data belum terkirim.
    pub acked: bool,
}

#[derive(Debug, Default, Clone)]
pub struct CycleReport {
    pub rejected: bool,
    pub upload: UploadReport,
    pub commands: Vec<Command>,
}

pub struct Machine {
    pub config: MachineConfig,
    client: Client,
    /// Ingatan log mesin.
    pub records: Vec<Record>,
    /// User di mesin: PIN → nama.
    pub users: BTreeMap<String, String>,
    /// Template di mesin: (PIN, nomor jari) → isi biner.
    pub templates: BTreeMap<(String, i16), Vec<u8>>,
    /// Opsi `SET OPTIONS ...` yang diterima.
    pub options_set: Vec<String>,
    pub reboots: u32,
    /// Stamp ATTLOG terakhir yang dikembalikan server di handshake.
    pub server_stamp: Option<i64>,
    latencies: LatencyStats,
}

impl Machine {
    pub fn new(authority: impl Into<String>, config: MachineConfig) -> Self {
        let client = Client::new(authority, config.user_agent.clone());
        Self {
            config,
            client,
            records: Vec::new(),
            users: BTreeMap::new(),
            templates: BTreeMap::new(),
            options_set: Vec::new(),
            reboots: 0,
            server_stamp: None,
            latencies: LatencyStats::default(),
        }
    }

    pub fn serial(&self) -> &str {
        &self.config.serial
    }

    pub fn add_record(&mut self, record: Record) {
        self.records.push(record);
    }

    /// Latensi dan galat yang terkumpul sejak panggilan terakhir.
    pub fn take_stats(&mut self) -> LatencyStats {
        std::mem::take(&mut self.latencies)
    }

    async fn timed(
        &mut self,
        response: impl Future<Output = Result<Response, SimError>>,
        started: Instant,
    ) -> Result<Response, SimError> {
        let outcome = response.await;
        match &outcome {
            Ok(_) => self.latencies.record(started.elapsed()),
            Err(_) => self.latencies.record_error(),
        }
        outcome
    }

    async fn get(&mut self, target: &str) -> Result<Response, SimError> {
        let started = Instant::now();
        let client = self.client.clone();
        let target = target.to_owned();
        self.timed(async move { client.get(&target).await }, started)
            .await
    }

    async fn post(&mut self, target: &str, body: &[u8]) -> Result<Response, SimError> {
        let started = Instant::now();
        let client = self.client.clone();
        let target = target.to_owned();
        let body = body.to_vec();
        self.timed(async move { client.post(&target, &body).await }, started)
            .await
    }

    /// `GET /iclock/cdata?SN=...&options=all&pushver=...&language=69`.
    pub async fn handshake(&mut self) -> Result<Handshake, SimError> {
        let mut target = format!(
            "/iclock/cdata?SN={}&options=all&pushver={}&language=69",
            self.config.serial, self.config.push_version
        );
        if let Some(device_type) = &self.config.device_type {
            target.push_str(&format!("&DeviceType={device_type}"));
        }

        let response = self.get(&target).await?;
        if response.status != 200 {
            return Err(SimError::Status(response.status));
        }
        let text = response.text();
        if text.trim().is_empty() {
            return Ok(Handshake::Rejected);
        }
        let options = Options {
            pairs: kv::pairs(&text, false),
        };
        self.server_stamp = options.attlog_stamp();
        Ok(Handshake::Accepted(options))
    }

    /// Log yang akan diunggah pada koneksi ini menurut [`Resend`].
    fn pending_records(&self) -> Vec<&Record> {
        match (self.config.resend, self.server_stamp) {
            (Resend::FromStamp, Some(stamp)) => self
                .records
                .iter()
                .filter(|record| record.stamp() > stamp)
                .collect(),
            _ => self.records.iter().collect(),
        }
    }

    /// Mengunggah log dalam request berurutan (`Stamp` = detik ZK log terakhir di batch itu).
    /// Berhenti pada jawaban yang bukan `OK`.
    pub async fn upload_records(&mut self) -> Result<UploadReport, SimError> {
        let pending: Vec<Record> = self.pending_records().into_iter().cloned().collect();
        let mut report = UploadReport {
            acked: true,
            ..UploadReport::default()
        };

        for chunk in pending.chunks(self.config.max_rows_per_request.max(1)) {
            let body: String = chunk
                .iter()
                .map(|record| {
                    format!(
                        "{}\t{}\t{}\t{}\t0\t0\t0\n",
                        record.pin,
                        record.time.format(TIME_FORMAT),
                        record.status,
                        record.verify
                    )
                })
                .collect();
            let stamp = chunk.iter().map(Record::stamp).max().unwrap_or_default();
            let target = format!(
                "/iclock/cdata?SN={}&table=ATTLOG&Stamp={stamp}",
                self.config.serial
            );

            let response = self.post(&target, body.as_bytes()).await?;
            report.requests += 1;
            if response.status != 200 || !response.text().starts_with("OK") {
                report.acked = false;
                break;
            }
            report.sent += chunk.len();
        }
        Ok(report)
    }

    /// Mengunggah template sidik jari lewat `table=OPERLOG` (`FP PIN=...`).
    pub async fn upload_templates(
        &mut self,
        templates: &[(String, i16, Vec<u8>)],
        op_stamp: i64,
    ) -> Result<Response, SimError> {
        let body: String = templates
            .iter()
            .map(|(pin, finger, data)| {
                format!(
                    "FP PIN={pin}\tFID={finger}\tSize={}\tValid=1\tTMP={}\n",
                    data.len(),
                    STANDARD.encode(data)
                )
            })
            .collect();
        let target = format!(
            "/iclock/cdata?SN={}&table=OPERLOG&OpStamp={op_stamp}",
            self.config.serial
        );
        self.post(&target, body.as_bytes()).await
    }

    /// `GET /iclock/getrequest?SN=...`: perintah tertunda (kosong bila server menjawab `OK`).
    pub async fn poll(&mut self) -> Result<Vec<Command>, SimError> {
        let response = self
            .get(&format!("/iclock/getrequest?SN={}", self.config.serial))
            .await?;
        if response.status != 200 {
            return Err(SimError::Status(response.status));
        }
        Ok(parse_commands(&response.text()))
    }

    /// Menjalankan perintah dan mengirim hasilnya. `INFO` dijawab dengan badan info perangkat;
    /// jawaban lain digabung dalam satu `POST /iclock/devicecmd`.
    pub async fn execute(&mut self, commands: &[Command]) -> Result<(), SimError> {
        let mut results = String::new();

        for command in commands {
            let text = command.text.as_str();
            if text == "INFO" {
                self.send_info(command.id).await?;
                continue;
            }

            let (name, code) = if text == "CHECK" {
                ("CHECK", 0)
            } else if text == "REBOOT" {
                self.reboots += 1;
                ("REBOOT", 0)
            } else if let Some(option) = text.strip_prefix("SET OPTIONS ") {
                self.options_set.push(option.to_owned());
                ("SET OPTIONS", 0)
            } else if let Some(fields) = text.strip_prefix("DATA USER ") {
                ("DATA", self.store_user(fields))
            } else if let Some(fields) = text.strip_prefix("DATA FP ") {
                ("DATA", self.store_template(fields))
            } else {
                (text.split_whitespace().next().unwrap_or("UNKNOWN"), -1)
            };
            results.push_str(&format!("ID={}&Return={code}&CMD={name}\n", command.id));
        }

        if !results.is_empty() {
            let target = format!("/iclock/devicecmd?SN={}", self.config.serial);
            let response = self.post(&target, results.as_bytes()).await?;
            if response.status != 200 {
                return Err(SimError::Status(response.status));
            }
        }
        Ok(())
    }

    async fn send_info(&mut self, id: i64) -> Result<(), SimError> {
        let mut body = format!(
            "ID={id}\nReturn=0\nCMD=INFO\nPushVersion={}\nUserCount={}\nFPCount={}\nFaceCount=0\n\
             TransactionCount={}\n~DeviceName=SimFace\n~Platform=SIM\nIPAddress=10.0.0.5\n",
            self.config.push_version,
            self.users.len(),
            self.templates.len(),
            self.records.len()
        );
        if let Some(version) = &self.config.fp_version {
            body.push_str(&format!("~ZKFPVersion={version}\n"));
        }
        let target = format!("/iclock/devicecmd?SN={}", self.config.serial);
        let response = self.post(&target, body.as_bytes()).await?;
        if response.status != 200 {
            return Err(SimError::Status(response.status));
        }
        Ok(())
    }

    /// `DATA USER PIN=..\tName=..\tPri=..`; mengembalikan kode `Return`.
    fn store_user(&mut self, fields: &str) -> i32 {
        let fields = tab_fields(fields);
        match (field(&fields, "PIN"), field(&fields, "Name")) {
            (Some(pin), Some(name)) if !pin.is_empty() => {
                self.users.insert(pin.to_owned(), name.to_owned());
                0
            }
            _ => -1,
        }
    }

    /// `DATA FP PIN=..\tFID=..\tSize=..\tValid=..\tTMP=..`: menolak bila ukuran tidak cocok,
    /// seperti firmware yang memeriksa panjang template.
    fn store_template(&mut self, fields: &str) -> i32 {
        let fields = tab_fields(fields);
        let parsed = (|| {
            let pin = field(&fields, "PIN")?;
            let finger = field(&fields, "FID")?.parse::<i16>().ok()?;
            let size = field(&fields, "Size")?.parse::<usize>().ok()?;
            let data = STANDARD.decode(field(&fields, "TMP")?).ok()?;
            (data.len() == size).then(|| (pin.to_owned(), finger, data))
        })();
        match parsed {
            Some((pin, finger, data)) => {
                self.templates.insert((pin, finger), data);
                0
            }
            None => -1,
        }
    }

    /// Satu koneksi seperti yang dilakukan firmware: handshake, unggah log, polling, jalankan
    /// perintah. Mesin yang ditolak berhenti setelah handshake.
    pub async fn cycle(&mut self) -> Result<CycleReport, SimError> {
        let mut report = CycleReport::default();

        if matches!(self.handshake().await?, Handshake::Rejected) {
            report.rejected = true;
            return Ok(report);
        }
        if !self.records.is_empty() {
            report.upload = self.upload_records().await?;
        }
        report.commands = self.poll().await?;
        let commands = report.commands.clone();
        self.execute(&commands).await?;
        Ok(report)
    }

    /// Menunggu sebelum koneksi berikutnya (mengikuti `Delay` dari server bila ada).
    pub fn delay_from(options: &Options) -> Duration {
        Duration::from_secs(
            options
                .get("Delay")
                .and_then(|value| value.parse().ok())
                .unwrap_or(60),
        )
    }
}

fn tab_fields(fields: &str) -> Vec<(&str, &str)> {
    fields
        .split('\t')
        .filter_map(|part| part.split_once('='))
        .collect()
}

fn field<'a>(fields: &[(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| *value)
}

/// `C:<id>:<teks>` per baris; selain itu (mis. `OK`) berarti tidak ada perintah.
fn parse_commands(body: &str) -> Vec<Command> {
    body.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("C:")?;
            let (id, text) = rest.split_once(':')?;
            Some(Command {
                id: id.parse().ok()?,
                text: text.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perintah_diurai_dan_ok_berarti_kosong() {
        assert!(parse_commands("OK").is_empty());
        assert!(parse_commands("").is_empty());

        let commands = parse_commands(
            "C:90470:CHECK\nC:5:SET OPTIONS MachineTZ=+0700\nC:6:DATA USER PIN=1\tName=A\tPri=0\nsampah\nC:x:BAD\n",
        );

        assert_eq!(commands.len(), 3);
        assert_eq!(
            commands[0],
            Command {
                id: 90470,
                text: "CHECK".to_owned()
            }
        );
        assert_eq!(commands[1].text, "SET OPTIONS MachineTZ=+0700");
        assert_eq!(commands[2].text, "DATA USER PIN=1\tName=A\tPri=0");
    }

    #[test]
    fn stamp_dari_opsi_handshake() {
        let basic = Options {
            pairs: kv::pairs("GET OPTION FROM:SN\nStamp=785168153\nDelay=30\n", false),
        };
        assert_eq!(basic.attlog_stamp(), Some(785_168_153));
        assert_eq!(Machine::delay_from(&basic), Duration::from_secs(30));

        let att = Options {
            pairs: kv::pairs("ATTLOGStamp=42\nStamp=7\n", false),
        };
        assert_eq!(att.attlog_stamp(), Some(42));

        let none = Options {
            pairs: kv::pairs("Stamp=None\n", false),
        };
        assert_eq!(none.attlog_stamp(), None);
        assert_eq!(Machine::delay_from(&none), Duration::from_secs(60));
    }

    #[test]
    fn mesin_menyimpan_user_dan_template_dengan_pemeriksaan_ukuran() {
        let mut machine = Machine::new("127.0.0.1:1", MachineConfig::new("SN"));

        assert_eq!(machine.store_user("PIN=12\tName=Budi Santoso\tPri=0"), 0);
        assert_eq!(machine.users["12"], "Budi Santoso");
        assert_eq!(machine.store_user("Name=Tanpa Pin"), -1);

        assert_eq!(
            machine.store_template("PIN=12\tFID=3\tSize=6\tValid=1\tTMP=AQIDBAUG"),
            0
        );
        assert_eq!(
            machine.templates[&("12".to_owned(), 3)],
            vec![1, 2, 3, 4, 5, 6]
        );
        assert_eq!(
            machine.store_template("PIN=12\tFID=4\tSize=7\tValid=1\tTMP=AQIDBAUG"),
            -1
        );
        assert_eq!(
            machine.store_template("PIN=12\tFID=4\tSize=6\tValid=1\tTMP=!!"),
            -1
        );
    }

    #[test]
    fn pemilihan_log_menurut_kebijakan_kirim_ulang() {
        use chrono::NaiveDate;
        let at = |day: u32| {
            NaiveDate::from_ymd_opt(2024, 6, day)
                .unwrap()
                .and_hms_opt(8, 0, 0)
                .unwrap()
        };
        let mut machine = Machine::new("127.0.0.1:1", MachineConfig::new("SN"));
        for day in [3, 4, 5] {
            machine.add_record(Record::new("1", at(day)));
        }

        // Semua dikirim bila kebijakan All atau server belum punya stamp.
        assert_eq!(machine.pending_records().len(), 3);
        machine.config.resend = Resend::FromStamp;
        assert_eq!(machine.pending_records().len(), 3);

        // FromStamp: hanya yang melebihi stamp server.
        machine.server_stamp = Some(zk_time::encode(at(4).and_utc()));
        let pending = machine.pending_records();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].time, at(5));
    }
}
