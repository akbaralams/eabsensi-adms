# eabsensi-adms

Server ADMS (protokol push iClock) untuk mesin absensi ZKTeco, ditulis ulang dalam Rust dari aplikasi Laravel lama `../eabsensi-99`. Bukan port baris per baris: perilaku protokol dipertahankan, kode dan skema dirapikan ke model data eAbsensi v2.

Pekerjaan berurutan ada di [TASKS.md](TASKS.md). Kerjakan satu task sampai kriteria selesainya terpenuhi, commit, lalu lanjut.

## Sistem empat repo (satu database PostgreSQL)
| Repo | Folder lokal | Peran |
|---|---|---|
| `eabsensi-v2` | `../eabsensi-v2` | Web operator (Laravel). **Pemilik skema** (migration), UI kelola mesin, dan aturan bisnis kehadiran. |
| `eabsensi-mobile-api` | `../eabsensi-mobile-api` | API mobile (Rust). Tidak berbagi kode dengan repo ini. |
| `eabsensi-adms` | repo ini | Protokol mesin: handshake, terima log, antrean perintah. |
| `eabsensi-mobile-app` | `../eabsensi-mobile-app` | Aplikasi Flutter. |

Rencana dan alasan pemisahan: `../eabsensi-v2/docs/rust-services-plan.md` (bagian "Batas dengan ADMS"). Aturan produk v2 ada di `../eabsensi-v2/CLAUDE.md`.

## Acuan lama (hanya dibaca, jangan diubah)
- **`../eabsensi-99`** (acuan utama): Laravel 5.x khusus ADMS. Rute `routes/web.php` grup `iclock` (`cdata` GET/POST, `getrequest`, `fdata`, `devicecmd`, `registry`, `push`, `ping`, `rtdata`, `querydata`), `app/Http/Controllers/IDeviceController.php` (±1.600 baris), model di `app/*.php` (`DeviceList`, `DeviceTemp`, `SerialList`, `CommandList`, `CommandsUser`, `EmployeeList`, `EmployeeDevice`, `EmployeeArea`, `EmployeeFP`, `TransactionLogs`, `OperationLogs`, `TransSuspects`), notifikasi `app/Services/SendNotif.php` dan `SendSuspectNotif.php`. Skemanya Oracle dan tidak ada di migration; nama tabel/kolom dibaca dari model dan query.
- `../eabsensi` (versi lebih lama, `IDeviceController.php` ±375 baris): hanya pembanding.
- Istilah lama (SKPD, SPMU, NRK, eTKD) tidak dipakai di kode dan skema baru; gunakan Department, Region, `employee_number`.

## Aturan yang tidak boleh dilanggar
- **Tidak ada migration di repo ini.** Tabel mesin (perangkat, perintah, log mentah, dst.) dibuat lewat migration di `eabsensi-v2` berdasarkan usulan skema dari Task 2. Skema dipin lewat `SCHEMA_VERSION` + dump `../eabsensi-v2/database/dumps/pgsql-schema.sql`.
- **Punch dari mesin masuk ke `attendance_punches`** dengan `source = device` (kasus enum baru di v2). Tidak ada engine proses batch: status harian dihitung v2 dari roster + punch.
- **Log mentah selalu disimpan** dan idempoten (kunci unik nomor seri + PIN + waktu); mesin mengirim ulang log, jadi penerimaan ganda tidak boleh membuat punch ganda.
- **Waktu mesin** adalah waktu lokal mesin (umumnya `Asia/Jakarta`); simpan UTC di database.
- **Balasan protokol** mengikuti apa yang diharapkan firmware (teks polos `OK`, baris `C:<id>:<perintah>`, opsi handshake); salah format = mesin berhenti mengirim. Perilaku ini dicocokkan dengan acuan lama dan diuji dengan simulator mesin.
- **Data biometrik** (template sidik jari/wajah, `EmployeeFP`) adalah data pribadi sensitif: hanya disimpan bila memang diputuskan user di Task 2, tidak pernah ditulis ke log.

## Versi (wajib terbaru)
Gunakan versi **stabil terbaru** untuk semuanya: toolchain Rust stable terbaru, dan setiap crate pada rilis stabil terbaru saat ditambahkan (cek crates.io/docs.rs, bukan dari ingatan). Pin versi di `Cargo.toml`, `Cargo.lock` di-commit. Saat mulai sesi baru, cek `cargo update --dry-run`/`cargo outdated` dan naikkan bila ada rilis baru.

## Environment (Windows)
- Rust (toolchain MSVC) dan sqlx-cli sudah terpasang, versinya dicatat di README.
- PostgreSQL 18 lokal `127.0.0.1:5432`, database dev `eabsensi_v2`. Kredensial di `../eabsensi-v2/.env`; salin ke `.env` repo ini (di-gitignore), jangan di-commit. `psql`/`pg_dump`: `C:\Program Files\PostgreSQL\18\bin`.
- Mesin ZKTeco fisik mungkin tidak tersedia saat pengembangan; simulator mesin (Task 6) adalah alat uji utama.

## Standar kode
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` hijau sebelum commit.
- Tidak ada `unwrap()`/`expect()` di jalur request. Parser protokol dipisah dari HTTP dan diuji unit dengan contoh payload nyata dari acuan lama.
- Query lewat `sqlx::query!`/`query_as!` (metadata offline `.sqlx/` di-commit), selalu dengan binding parameter.
- Konfigurasi hanya dari environment.

## Git
- Conventional Commits, satu commit per task/perubahan utuh beserta test-nya.
- Jangan `git push` kecuali diminta. Jangan commit `.env` atau kredensial.
