//! Penerimaan log absensi: log mentah idempoten, penentuan hasil, dan pembuatan punch.

pub mod batch;
pub mod biometrics;
pub mod outcome;

pub use batch::{Summary, ingest_attlog};
