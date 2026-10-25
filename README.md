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
