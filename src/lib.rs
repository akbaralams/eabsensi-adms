//! Server ADMS (protokol push iClock) untuk mesin absensi ZKTeco.

pub mod cli;
pub mod config;
pub mod db;
pub mod http;
pub mod ingest;
pub mod offline;
pub mod protocol;
pub mod rekey;
pub mod simulator;
pub mod state;
pub mod vault;
