# eabsensi-adms

Lihat [CLAUDE.md](CLAUDE.md) untuk konteks dan aturan, [TASKS.md](TASKS.md) untuk urutan pekerjaan.

## Toolchain

Dipasang di Windows (host MSVC), dicatat 2026-10-25:

| Alat | Versi |
|---|---|
| `rustc` | 1.99.1 (58cf8f9c3 2026-10-13) |
| `cargo` | 1.99.1 (5f94df478 2026-08-27) |
| `sqlx` (sqlx-cli, fitur `rustls,postgres`) | 0.9.0 |
| Visual Studio Build Tools | 2026 18.11 (MSVC 14.51), workload "Desktop development with C++" |

Pasang ulang `sqlx-cli`:

```bash
cargo install sqlx-cli --no-default-features --features rustls,postgres
```
