//! Penerimaan log absensi: log mentah idempoten, penentuan hasil, dan pembuatan punch.

pub mod batch;
pub mod outcome;

pub use batch::{Summary, ingest_attlog};
