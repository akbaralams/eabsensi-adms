//! Simulator mesin ZKTeco untuk uji end-to-end dan uji beban (Task 6).
//!
//! Meniru perilaku mesin yang tercatat di acuan lama: handshake, unggah ATTLOG (termasuk
//! mengirim ulang seluruh batch pada tiap koneksi), polling perintah, dan pengiriman hasil
//! perintah, lewat koneksi TCP sungguhan dengan bentuk request yang sama dengan log produksi.
//! Perilaku firmware yang belum terbukti (mis. apakah stamp menghentikan kiriman ulang) dapat
//! dipilih lewat [`Resend`] agar kedua kemungkinan diuji.

mod http;
mod machine;
mod stats;

pub use http::{Client, Response};
pub use machine::{
    Command, CycleReport, Handshake, Machine, MachineConfig, Options, Record, Resend, UploadReport,
};
pub use stats::{LatencyStats, StatsReport};

#[derive(Debug, thiserror::Error)]
pub enum SimError {
    #[error("galat jaringan: {0}")]
    Io(#[from] std::io::Error),
    #[error("request melewati batas waktu")]
    Timeout,
    #[error("respons tidak valid: {0}")]
    BadResponse(String),
    #[error("server menjawab status {0}")]
    Status(u16),
}
