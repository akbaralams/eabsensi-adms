//! Protokol iClock/ADMS murni: tidak tahu HTTP maupun database, hanya teks masuk dan teks keluar.
//! Format balasan mengikuti `docs/legacy-analysis.md` dan dicocokkan byte-per-byte dengan
//! fixture yang dibangkitkan dari kode PHP lama (`tests/fixtures/`).

pub mod command;
pub mod handshake;
pub mod info;
pub mod kv;
pub mod zk_time;
