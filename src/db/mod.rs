//! Akses database. Semua query lewat `sqlx::query!` dengan binding parameter; metadata offline
//! ada di `.sqlx/` (perbarui dengan `cargo sqlx prepare`).

pub mod commands;
pub mod devices;
pub mod logs;
