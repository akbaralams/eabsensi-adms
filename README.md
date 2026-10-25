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

## Menjalankan

Butuh PostgreSQL dengan skema v2 (tabel mesin ada di dump yang dipin `SCHEMA_VERSION`; di mesin dev, database `eabsensi_v2` setelah `php artisan migrate` di `eabsensi-v2`).

```bash
cp .env.example .env     # isi DATABASE_URL
cargo run                # melayani http://0.0.0.0:8090
curl http://127.0.0.1:8090/healthz   # {"status":"ok","version":"..."}
curl http://127.0.0.1:8090/readyz    # 200 bila database terjangkau dan tabel devices ada, selain itu 503
```

Konfigurasi hanya dari environment, daftar variabel dan nilai bawaannya ada di [.env.example](.env.example). Log berformat JSON ke stdout (`RUST_LOG` mengatur level). Setiap request dibatasi ukuran body (`ADMS_MAX_BODY_BYTES`, 413 bila lebih) dan waktu (`ADMS_REQUEST_TIMEOUT_SECS`, 408 bila lewat).

## Kualitas dan CI

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test                 # butuh DATABASE_URL (satu uji memeriksa /readyz ke database sungguhan)
cargo sqlx prepare --check # metadata offline `.sqlx/` harus mutakhir; ubah query lalu jalankan `cargo sqlx prepare`
```

Build tanpa database memakai metadata `.sqlx/` (`SQLX_OFFLINE=true`). CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) menjalankan Postgres 18, memuat dump skema dari `eabsensi-v2` pada commit di `SCHEMA_VERSION` (repo itu privat bila perlu secret `V2_REPO_TOKEN` dengan hak baca), lalu `fmt`, `sqlx prepare --check`, `clippy`, dan `test`.

### Catatan Windows: "error reading dotenv file"
Makro `sqlx` 0.9 membaca berkas `.env` di **semua folder induk** proyek dan gagal bila salah satunya tidak valid. Di mesin ini `C:\laragon\www\.env` (sisa Laravel lama) berisi baris yang tidak bisa diurai, sehingga `cargo build` dari `C:\laragon\www\eabsensi-adms` gagal. Perbaiki atau pindahkan berkas induk itu, atau petakan repo ke drive lain dan bangun dari sana:

```powershell
subst X: C:\laragon\www\eabsensi-adms   # lalu: cd X:\ ; cargo build
```

## Simulator mesin

`src/simulator` (pustaka) dan `adms-sim` (biner) meniru mesin ZKTeco lewat TCP sungguhan dengan bentuk request yang sama dengan log produksi lama: handshake, unggah ATTLOG (termasuk kirim ulang dan batch besar), polling perintah, hasil perintah, dan unggah template. Dipakai oleh uji end-to-end (`tests/e2e.rs`) dan uji beban (`tests/simulator_load.rs`, hasil di [docs/load-test.md](docs/load-test.md)).

```bash
# mesin harus sudah terdaftar di tabel devices (D2); --pin = employee_number pegawai yang ada
cargo run --bin adms-sim -- --url http://127.0.0.1:8090 --serial SN-0001 --rows 20 --cycles 3 --pin 12345
cargo run --bin adms-sim -- --url http://127.0.0.1:8090 --serial LOAD --machines 100 --cycles 20 --rows 20 --pin 12345
```

Opsi: `--machines N` (serial menjadi `<serial>-0000`, ...), `--cycles N`, `--rows N`, `--delay-ms MS`, `--resend all|stamp` (apakah mesin mengirim ulang seluruh ingatan tiap koneksi seperti yang tercatat di log lama, atau hanya yang melewati stamp server), `--device-type att`. Keluar dengan status non-nol bila ada galat jaringan atau status server.

Mendaftarkan mesin simulasi di database dev:

```sql
INSERT INTO devices (serial_number, name, created_at, updated_at)
SELECT 'SN-' || lpad(n::text, 4, '0'), 'Simulasi', now(), now() FROM generate_series(1, 3) n;
```

Perilaku firmware yang belum terbukti (mis. apakah stamp menghentikan pengiriman ulang) diuji pada kedua kemungkinan; simulator bukan pengganti uji di mesin fisik.

## Deploy

Ringkasan; perincian, alasan, dan pemecahan masalah ada di [docs/operations.md](docs/operations.md).

1. **Skema dulu.** Terapkan migration `eabsensi-v2` sampai commit di berkas `SCHEMA_VERSION` (ADMS tidak punya migration).
2. **Role database** `eabsensi_adms` dengan hak minimal (daftar `GRANT` di operations.md bagian 3).
3. **Build** di mesin build: `SQLX_OFFLINE=true cargo build --release --locked`, salin `target/release/eabsensi-adms` ke server.
4. **Konfigurasi lewat environment** (daftar di [.env.example](.env.example)): `DATABASE_URL` wajib; `ADMS_BIND_ADDR` diikat ke antarmuka jaringan mesin; `ADMS_ATTENDANCE_TIMEZONE` sama dengan v2; `BIOMETRIC_KEY` + `BIOMETRIC_KEY_ID` bila template biometrik dipakai (tanpa kunci biometrik dinonaktifkan).
5. **Jalankan sebagai service**: systemd ([deploy/eabsensi-adms.service](deploy/eabsensi-adms.service)) atau NSSM/WinSW di Windows (belum diuji; contoh di operations.md bagian 7).
6. **Jaringan**: izinkan port server hanya dari subnet mesin (LAN/VPN). Protokol mesin tanpa TLS dan tanpa autentikasi selain nomor seri; jangan buka ke internet.
7. **Mesin**: daftarkan serial di UI v2 lebih dulu, lalu isi alamat dan port server di menu mesin (*COMM. → Cloud Server Setting*). Mesin yang belum terdaftar ditolak tetapi menyimpan datanya.

### Pemeriksaan setelah deploy

| Langkah | Hasil yang diharapkan |
|---|---|
| `curl http://<host>:<port>/healthz` | `{"status":"ok","version":"..."}` |
| `curl -i http://<host>:<port>/readyz` | `200` `{"status":"ready"}` (503 = database/skema/hak bermasalah) |
| Log start (JSON) | `"eabsensi-adms melayani"`; tanpa `WARN` tak terduga |
| `eabsensi-adms check-offline` | tanpa galat database (mesin baru wajar terdaftar sebagai offline) |
| `adms-sim --url http://<host>:<port> --serial <SN terdaftar> --rows 5 --cycles 2 --pin <employee_number>` | `0 galat`, 5 log menjadi 5 punch di v2 |
| Satu mesin fisik, satu absensi | `device_logs.outcome = punched`, punch `source = device`, tampil di kehadiran harian v2; tidak ada punch ganda setelah jaringan diputus-sambung |

Uji dengan mesin fisik **belum dilakukan** (tidak tersedia saat pengembangan); panduan pengujian pertamanya di [docs/field-test-checklist.md](docs/field-test-checklist.md).

## Perintah operasional

Biner yang sama menjalankan server (tanpa argumen atau `serve`) dan subperintah berikut (membaca `DATABASE_URL` dari environment; `--help` untuk ringkasan):

```bash
eabsensi-adms check-offline [--factor 3] [--min-seconds 120]   # status keluar 0 online semua, 2 ada mesin offline, 1 galat
eabsensi-adms rekey-biometrics [--from ID_KUNCI] [--dry-run]    # rotasi kunci biometrik; 0 sukses, 2 ada baris tak bisa dibuka
```

Jadwalkan `check-offline` (cron/Task Scheduler) untuk peringatan mesin offline; prosedur rotasi kunci, pencadangan kunci, masa simpan log (usulan 24 bulan), dan pola log yang perlu diberi peringatan ada di operations.md bagian 9, 12, dan 13.
