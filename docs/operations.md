# Operasional eabsensi-adms

Panduan memasang, menjalankan, memantau, dan merawat server ADMS. Ringkasan langkah deploy ada di [README.md](../README.md); dokumen ini memuat alasan dan perinciannya.

**Status pengujian.** Semua yang berhubungan dengan kode (build rilis, `/healthz`, `/readyz`, `check-offline`, `rekey-biometrics`, simulator) sudah dijalankan di mesin pengembangan Windows. Yang **belum diuji** dan ditandai "belum diuji" di bawah: unit systemd, pembungkus service Windows, aturan firewall, reverse proxy, role Postgres terpisah, dan seluruh jalur dengan **mesin fisik** (tidak tersedia saat pengembangan; lihat [field-test-checklist.md](field-test-checklist.md)).

## 1. Gambaran

```
Mesin ZKTeco ── HTTP polos (LAN/VPN) ──▶ eabsensi-adms ──▶ PostgreSQL (satu database dengan eabsensi-v2)
                                           │ (satu proses, port sendiri)
                                           └─ log JSON ke stdout
```

- Satu biner (`eabsensi-adms`, ±7 MB) tanpa dependensi runtime selain PostgreSQL. Biner kedua `adms-sim` (simulator) dipasang di server staging untuk uji asap.
- **Skema dimiliki `eabsensi-v2`.** ADMS tidak punya migration; versi skema yang diuji ADMS tercatat di berkas `SCHEMA_VERSION` (commit `eabsensi-v2`). Terapkan migration v2 lebih dulu, baru deploy ADMS (bagian 11).
- Protokol mesin **tanpa TLS dan tanpa autentikasi** selain nomor seri terdaftar. Karena itu server hanya boleh terjangkau dari jaringan mesin (LAN atau VPN), bukan internet.

## 2. Prasyarat

| Kebutuhan | Keterangan |
|---|---|
| PostgreSQL | Versi yang sama dengan v2 (diuji pada 18); skema v2 sudah termigrasi sampai `SCHEMA_VERSION`. |
| Host | Windows Server atau Linux x86-64. Memori kecil (puluhan MB); CPU hampir tidak terpakai pada polling normal ([load-test.md](load-test.md)). |
| Jaringan | Mesin dapat menjangkau `host:port` server lewat TCP; server dapat menjangkau PostgreSQL. |
| Build | Toolchain Rust stable dan (Windows) Visual Studio Build Tools hanya di mesin build, bukan di server. |

## 3. Role database

ADMS cukup memakai role sendiri tanpa hak DDL dan tanpa akses ke `users`. Daftar hak di bawah **diturunkan dari seluruh query `sqlx` di kode** (`.sqlx/`): setiap tabel hanya dengan operasi yang benar-benar dipakai, tanpa `DELETE`. **Belum diuji** dengan role terpisah (role pengembangan tidak punya hak membuat role); setelah membuatnya, jalankan uji asap bagian 8 sebelum mesin pertama dihubungkan.

```sql
CREATE ROLE eabsensi_adms LOGIN PASSWORD '<kata sandi kuat>';
GRANT CONNECT ON DATABASE eabsensi_v2 TO eabsensi_adms;
GRANT USAGE ON SCHEMA public TO eabsensi_adms;

GRANT SELECT ON employees, periods                              TO eabsensi_adms;
GRANT SELECT, UPDATE ON devices                                 TO eabsensi_adms;
GRANT SELECT, INSERT, UPDATE ON device_commands, device_logs,
                                biometric_templates             TO eabsensi_adms;
GRANT SELECT, INSERT ON attendance_punches                      TO eabsensi_adms;

GRANT USAGE, SELECT ON SEQUENCE device_commands_id_seq, device_logs_id_seq,
                                biometric_templates_id_seq, attendance_punches_id_seq
                                                                TO eabsensi_adms;
```

`DATABASE_URL=postgres://eabsensi_adms:<kata sandi>@<host>:5432/eabsensi_v2`. Bila nama tabel/urutan berbeda di rilis v2 berikutnya, cocokkan dengan `database/dumps/pgsql-schema.sql`. Subperintah `rekey-biometrics` dan `check-offline` memakai hak yang sama.

## 4. Build dan pemasangan

```bash
# di mesin build, pada commit yang akan dirilis
SQLX_OFFLINE=true cargo build --release --locked      # ±1 menit; hasil di target/release/
./target/release/eabsensi-adms --version
```

`SQLX_OFFLINE=true` memakai metadata `.sqlx/` yang sudah di-commit sehingga build tidak butuh database. Salin `eabsensi-adms` (dan `adms-sim` bila perlu) ke server, mis. `/opt/eabsensi-adms/` atau `C:\eabsensi-adms\`. Simpan biner versi sebelumnya untuk rollback.

## 5. Konfigurasi

Semua dari environment (tidak ada berkas konfigurasi); `.env` hanya dibaca bila ada di folder kerja (pengembangan). Nilai bawaan ada di [.env.example](../.env.example).

| Variabel | Wajib | Bawaan | Keterangan |
|---|---|---|---|
| `DATABASE_URL` | ya | | Berisi kata sandi: jangan di baris perintah atau log. |
| `ADMS_BIND_ADDR` | | `0.0.0.0:8090` | Ikat ke antarmuka jaringan mesin saja (mis. `10.15.64.10:8090`), bukan semua antarmuka, bila server punya beberapa jaringan. |
| `ADMS_MAX_BODY_BYTES` | | 8 MiB | Batch ATTLOG 1.000 baris ≈ 45 KB, jauh di bawah ini. |
| `ADMS_REQUEST_TIMEOUT_SECS` | | 30 | Batch 1.000 baris ≈ 1,2 detik ([load-test.md](load-test.md)). |
| `ADMS_DB_MAX_CONNECTIONS` / `ADMS_DB_ACQUIRE_TIMEOUT_SECS` | | 10 / 5 | Naikkan bila ratusan mesin polling rapat; pastikan `max_connections` PostgreSQL cukup. |
| `ADMS_COMMAND_ACK_TIMEOUT_SECS` / `ADMS_COMMAND_MAX_ATTEMPTS` | | 120 / 3 | Perintah tanpa jawaban dikirim ulang, lalu `failed`. |
| `ADMS_ATTENDANCE_TIMEZONE` | | `Asia/Jakarta` | **Harus sama** dengan `ATTENDANCE_TIMEZONE` di v2 (penentu bulan periode terkunci). |
| `ADMS_MIN_LOG_TIME` / `ADMS_SUSPECT_FUTURE_SECS` | | `2020-01-01 00:00:00` / 500 | Batas bawah log dan ambang `is_suspect`. |
| `BIOMETRIC_KEY`, `BIOMETRIC_KEY_ID`, `BIOMETRIC_OLD_KEYS` | tidak, tetapi tanpa `BIOMETRIC_KEY` biometrik **dinonaktifkan** | | Lihat bagian 9. |
| `BIOMETRIC_STORE_FACE_PHOTO` | | `false` | Simpan juga foto wajah. |
| `RUST_LOG` | | `info` | Level log, mis. `eabsensi_adms=debug,info`. |

Konfigurasi yang tidak valid menghentikan start dengan pesan yang menyebut nama variabelnya (nilai rahasia tidak pernah dicetak). Sebelum start sungguhan, `eabsensi-adms --version` dan `eabsensi-adms check-offline` memvalidasi `DATABASE_URL` dan koneksinya.

## 6. Jaringan, port, firewall

### Port
- Firmware lama menghubungi **port yang dikonfigurasi di mesin** (log produksi lama: `Host: <ip>:80`). Dua pilihan:
  1. **Port sendiri** (bawaan 8090): ubah port server di menu mesin. Paling sederhana bila semua mesin bisa diubah.
  2. **Port 80** untuk mesin yang tidak bisa diubah. Jangan menaruhnya langsung di host yang sudah memakai port 80 (mis. Laragon/Apache/nginx untuk web v2). Pakai alamat IP kedua, host terpisah, atau reverse proxy yang meneruskan vhost mesin ke `127.0.0.1:8090`.
- Di Linux, `ADMS_BIND_ADDR=10.15.64.10:80` bekerja tanpa root lewat `AmbientCapabilities=CAP_NET_BIND_SERVICE` di unit systemd.

### Firewall (belum diuji)
Izinkan masuk ke port server **hanya dari subnet mesin**; tolak sisanya. Server tidak punya autentikasi selain nomor seri, jadi siapa pun yang bisa menjangkau port itu dapat menebak serial dan mengirim log.

```powershell
# Windows (jalankan sebagai Administrator); sesuaikan port dan subnet mesin
New-NetFirewallRule -DisplayName "eabsensi-adms mesin" -Direction Inbound -Protocol TCP `
  -LocalPort 8090 -RemoteAddress 10.15.64.0/22 -Action Allow
```

```bash
# Linux (ufw)
sudo ufw allow from 10.15.64.0/22 to any port 8090 proto tcp
```

### VPN ke jaringan mesin
Mesin di kantor jauh menjangkau server lewat VPN site-to-site; pastikan MTU aman (batch ATTLOG hingga puluhan KB) dan tidak ada NAT yang mengacak alamat sumber bila `device_logs.source_ip` dipakai untuk penelusuran. Tidak perlu membuka port ke internet.

### Reverse proxy (opsional, belum diuji)
Bila dipakai (mis. nginx di port 80): teruskan ke `http://127.0.0.1:8090`, `client_max_body_size 8m`, `proxy_read_timeout 35s`, `proxy_http_version 1.1`. Catatan: `device_logs.source_ip` akan berisi alamat proxy karena `X-Forwarded-For` belum dibaca server.

## 7. Menjalankan sebagai service

Server menulis log JSON ke stdout dan berhenti rapi pada Ctrl+C/SIGTERM (menunggu request berjalan selesai). Tidak menulis berkas apa pun.

### Linux (systemd) — belum diuji
Unit siap pakai: [deploy/eabsensi-adms.service](../deploy/eabsensi-adms.service) (pengguna sistem terpisah, `EnvironmentFile` mode 0600, pengerasan `ProtectSystem=strict` dst., `Restart=on-failure`). Perintah pemasangannya ada di komentar berkas itu. Log: `journalctl -u eabsensi-adms -f`.

### Windows — belum diuji
Biner bukan service Windows asli, jadi dibungkus. Rekomendasi: [NSSM](https://nssm.cc/) atau WinSW (keduanya perangkat pihak ketiga yang perlu Anda setujui dan pasang sendiri). Contoh dengan NSSM:

```powershell
nssm install eabsensi-adms C:\eabsensi-adms\eabsensi-adms.exe serve
nssm set eabsensi-adms AppDirectory C:\eabsensi-adms
nssm set eabsensi-adms AppEnvironmentExtra "DATABASE_URL=postgres://..." "ADMS_BIND_ADDR=10.15.64.10:8090" "BIOMETRIC_KEY_ID=k1"
nssm set eabsensi-adms AppStdout C:\eabsensi-adms\logs\adms.log
nssm set eabsensi-adms AppStderr C:\eabsensi-adms\logs\adms.log
nssm set eabsensi-adms AppRotateFiles 1
nssm set eabsensi-adms AppRotateBytes 52428800
nssm set eabsensi-adms AppExit Default Restart
nssm set eabsensi-adms Start SERVICE_AUTO_START
nssm start eabsensi-adms
```

`AppEnvironmentExtra` tersimpan di registry (terbaca oleh Administrator). Untuk `BIOMETRIC_KEY` lebih baik pakai akun layanan khusus dan batasi ACL kunci registry `HKLM\SYSTEM\CurrentControlSet\Services\eabsensi-adms\Parameters` ke akun itu dan Administrator.

## 8. Konfigurasi di mesin

1. **Daftarkan mesin di UI v2 lebih dulu** (nomor seri ada di menu mesin: *System Info → Device Info → Serial Number*). Mesin yang belum terdaftar atau dinonaktifkan **ditolak** (balasan kosong) dan **tetap menyimpan datanya**, lalu mengirim ulang setelah didaftarkan (keputusan D2; perilaku firmware ini belum teruji pada mesin fisik).
2. Di menu mesin: *COMM. → Cloud Server Setting* (nama menu berbeda antar firmware; kadang *ADMS*): **Server Address** = IP atau nama server, **Server Port** = port bagian 6, *Enable Domain Name* mati bila memakai IP, *Enable Proxy Server* mati, HTTPS mati (tidak didukung).
3. Atur **tanggal, jam, dan zona waktu mesin** sesuai lokasi. Zona di `devices.timezone` (v2) harus sama dengan zona lokal mesin: waktu log dikirim mesin sebagai waktu lokal dan dikonversi ke UTC memakai zona itu. ADMS juga mengirim `SET TIME`/`rtdata` untuk menyelaraskan jam.
4. Panjang PIN di mesin tidak boleh melebihi batas firmware dan harus sama dengan `employee_number` pegawai (dicocokkan persis; v2 membatasi default ke angka maksimal 9 karakter).
5. Simpan; mesin mulai handshake dalam hitungan detik. Verifikasi dengan bagian 10.

## 9. Kunci biometrik (Q7)

Template sidik jari/wajah disimpan **terenkripsi** (AES-256-GCM); hanya ADMS yang memegang kunci. Kunci tidak ada di database, kode, atau log.

### Membuat dan menyimpan
```bash
openssl rand -base64 32          # Linux/macOS/Git Bash
```
```powershell
[Convert]::ToBase64String((1..32 | ForEach-Object { Get-Random -Maximum 256 }) -as [byte[]])   # PowerShell
```
- Isi `BIOMETRIC_KEY` (base64) dan `BIOMETRIC_KEY_ID` (mis. `k2026a`, 1-16 karakter huruf/angka/`_`/`-`; **jangan dipakai ulang** untuk kunci lain).
- Tempat penyimpanan: berkas env mode 0600 milik root (systemd) atau registry service dengan ACL terbatas (Windows). **Bukan** di repo, tiket, atau chat.
- **Cadangan**: simpan salinan kunci (beserta id-nya) di tempat terpisah dari cadangan database, mis. pengelola kata sandi tim atau brankas fisik, dengan dua orang pemegang. Cadangan database saja **tidak cukup**; cadangan kunci saja juga tidak berguna tanpa database.
- **Bila kunci hilang**: template yang tersimpan tidak bisa dipulihkan. Mesin tetap memegang salinannya sendiri; daftarkan ulang atau biarkan mesin mengirim ulang templatenya setelah kunci baru dipasang (template baru ikut tersimpan saat mesin mengirim ulang batch OPERLOG). Tanpa `BIOMETRIC_KEY` ADMS tidak menyimpan dan tidak mengirim template apa pun.

### Rotasi
1. Buat kunci baru `K2` dengan id baru. Konfigurasikan: `BIOMETRIC_KEY=<K2>`, `BIOMETRIC_KEY_ID=<id2>`, `BIOMETRIC_OLD_KEYS=<id1>=<K1>` (kunci lama hanya untuk membaca). Restart service.
2. Periksa: `eabsensi-adms rekey-biometrics --from <id1> --dry-run`. Semua baris harus "bisa dibuka"; bila ada yang gagal, kunci lama belum lengkap di `BIOMETRIC_OLD_KEYS`.
3. Jalankan: `eabsensi-adms rekey-biometrics --from <id1>`. Aman diulang dan aman bila terhenti di tengah (per 200 baris satu transaksi; baris yang ditimpa mesin selama proses dilewati).
4. Pastikan tidak ada baris tersisa: `SELECT key_id, count(*) FROM biometric_templates GROUP BY key_id;` hanya memuat `<id2>`.
5. Hapus `<id1>` dari `BIOMETRIC_OLD_KEYS`, restart, lalu musnahkan salinan kunci lama sesuai kebijakan.

### Penghapusan
Menghapus pegawai di v2 menghapus template-nya (`ON DELETE CASCADE`); v2 juga punya aksi hapus template per pegawai. **Kebijakan masa simpan template dan persetujuan pegawai (UU PDP) belum ditetapkan pemilik produk (Q6)**; sampai itu ada, hapus manual saat pegawai berhenti.

## 10. Pemeriksaan setelah deploy

Urutkan; berhenti dan selidiki bila ada yang tidak sesuai.

| # | Langkah | Hasil yang diharapkan |
|---|---|---|
| 1 | `curl -s http://<host>:<port>/healthz` | `{"status":"ok","version":"<versi>"}` |
| 2 | `curl -s -i http://<host>:<port>/readyz` | `200` dan `{"status":"ready"}` (`503` = database tak terjangkau atau tabel `devices` tidak ada: migration v2 belum diterapkan atau hak role kurang) |
| 3 | Log start | baris JSON `"eabsensi-adms melayani"` dengan alamat dan versi; tidak ada `WARN` kecuali `BIOMETRIC_KEY tidak diisi` bila memang sengaja |
| 4 | `eabsensi-adms check-offline` | daftar mesin aktif yang belum terhubung (wajar untuk deploy baru); tidak ada galat database |
| 5 | Uji asap dengan simulator pada mesin uji terdaftar: `adms-sim --url http://<host>:<port> --serial <SN> --rows 5 --cycles 2 --pin <employee_number>` | `0 galat`, `0 mesin ditolak`; 5 baris menjadi log dan punch di v2 |
| 6 | Hubungkan **satu mesin fisik** (bagian 8) | muncul online di UI v2 dalam ±2× `poll_delay_seconds`; kolom info mesin (firmware, jumlah user) terisi setelah INFO |
| 7 | Lakukan satu absensi di mesin itu | log di `device_logs` dengan `outcome = punched`, punch `source = device`, dan tampil di kehadiran harian v2 |
| 8 | Ulangi langkah 7 setelah memutus lalu menyambung jaringan mesin | tidak ada punch ganda |

Bila belum ada mesin fisik, langkah 6-8 ditunda dan langkah 5 menjadi satu-satunya bukti; catat itu pada catatan rilis.

## 11. Pembaruan dan rollback

1. Baca `SCHEMA_VERSION` pada rilis ADMS baru; pastikan migration v2 sampai commit itu sudah diterapkan di database tujuan (v2 memakai pola expand/contract: kolom tidak dihapus sebelum ADMS berhenti membacanya, jadi ADMS lama tetap jalan di skema baru).
2. Jalankan uji asap bagian 10 di staging dengan biner baru.
3. Hentikan service, ganti biner, mulai lagi. Mesin yang sedang mengirim akan mengulang (log idempoten, tidak ada duplikat); henti sejenak aman.
4. **Rollback**: kembalikan biner sebelumnya. Skema tidak perlu dikembalikan (hanya penambahan).

## 12. Pemantauan

ADMS tidak mengekspor metrik (tanpa Prometheus); pemantauan lewat tiga hal:

1. **Mesin offline**: `eabsensi-adms check-offline` (status keluar `0` semua online, `2` ada offline, `1` galat). Jadwalkan tiap 5-10 menit dan kirim peringatan pada status `2`.
   - Cron: `*/10 * * * * /opt/eabsensi-adms/eabsensi-adms check-offline || /usr/local/bin/kirim-peringatan "ADMS: ada mesin offline"` (muat `DATABASE_URL` dari berkas env yang sama).
   - Task Scheduler (Windows): tindakan menjalankan `eabsensi-adms.exe check-offline`; kondisikan peringatan pada kode keluar 2.
   - Batas: `max(3 × poll_delay_seconds, 120)` detik per mesin (ubah dengan `--factor`/`--min-seconds`); mesin nonaktif tidak dihitung. UI v2 memakai 3 × jeda polling **tanpa** batas minimum (jeda terpendek 10 detik berarti batas 30 detik); perintah ini menambah minimum 120 detik agar peringatan tidak berkedip untuk mesin berjeda sangat pendek, jadi daftar keduanya bisa berbeda sesaat.
2. **Liveness/readiness**: pantau `GET /healthz` (proses hidup) dan `GET /readyz` (database) dari pemantau uptime.
3. **Log**: pola yang layak diberi peringatan:

| Pola di log | Arti | Tindakan |
|---|---|---|
| `"level":"ERROR"` + `galat database` | database bermasalah; mesin menerima 500 dan akan mengulang | periksa PostgreSQL dan kolam koneksi |
| `WARN` + `mesin tidak terdaftar atau nonaktif` | mesin menghubungi tetapi ditolak | daftarkan atau aktifkan di v2 (serial ada di kolom `serial`) |
| `WARN` + `perintah tidak dapat dirakit` / `perintah kirim sidik jari gagal` | perintah operator gagal; alasan juga di UI v2 | lihat `result_message` perintah |
| `ERROR` + `template tidak dapat didekripsi` | kunci biometrik hilang/salah di konfigurasi | periksa `BIOMETRIC_KEY*` |
| `INFO` + `ATTLOG diterima` dengan `rusak > 0` | baris log yang tidak terbaca (firmware/format baru?) | kumpulkan contoh lewat packet capture ([field-test-checklist.md](field-test-checklist.md)) |
| `INFO` + `ATTLOG diterima` dengan `suspect > 0` | jam mesin menyimpang | atur jam mesin; tinjau di UI v2 |

Log hanya memuat nomor seri, jumlah, dan alasan singkat. PIN, isi template, kunci, dan kata sandi **tidak pernah** dicatat.

## 13. Masa simpan log mentah (Q5)

**Usulan** (butuh persetujuan pemilik produk): simpan `device_logs` **24 bulan**, lalu hapus. Punch di `attendance_punches` tidak ikut terhapus (tautan `punch_id` bersifat `SET NULL`). Menghapus log mentah juga menghapus kunci idempotensinya; bila mesin kelak mengirim ulang log setua itu, log kembali masuk tetapi aturan `duplicate` (punch `approved` sama sudah ada) mencegah punch ganda, sedangkan log yang punch-nya sudah ditinjau manual bisa muncul lagi sebagai `unknown_pin`/`period_closed`.

Hapus berkala dalam potongan agar tidak mengunci tabel (jalankan sebagai pemilik skema, bukan role ADMS yang tidak punya `DELETE`):

```sql
-- pratinjau
SELECT count(*), min(received_at) FROM device_logs WHERE received_at < now() - interval '24 months';

-- hapus per 10.000 baris; ulangi sampai 0 baris terhapus
DELETE FROM device_logs
 WHERE id IN (SELECT id FROM device_logs
               WHERE received_at < now() - interval '24 months'
               ORDER BY id LIMIT 10000);
```

`device_commands` yang selesai dan lebih tua dari 12 bulan boleh dihapus dengan cara yang sama (`WHERE status IN ('succeeded','failed','cancelled') AND completed_at < now() - interval '12 months'`).

## 14. Cadangan dan pemulihan

- Cadangan harian database `eabsensi_v2` (`pg_dump -Fc`) sudah mencakup semua tabel mesin. Template di dalamnya terenkripsi; **kunci dicadangkan terpisah** (bagian 9).
- Pemulihan: pulihkan database, pasang biner dan konfigurasi (termasuk kunci dengan id yang sama), jalankan uji asap. Mesin mengirim ulang backlog-nya; log idempoten.

## 15. Pemecahan masalah

| Gejala | Kemungkinan penyebab | Periksa |
|---|---|---|
| Mesin tidak pernah muncul online | firewall/rute, alamat atau port salah di mesin, server tidak jalan | `curl http://<host>:<port>/healthz` dari jaringan mesin; menu COMM. mesin; log |
| Log: `mesin tidak terdaftar atau nonaktif` | serial belum didaftarkan atau `is_active` mati | UI v2 → mesin |
| Mesin online tetapi log absensi tidak masuk | belum terdaftar (data ditahan di mesin), atau bukan firmware yang dikenali | log server; field-test checklist; packet capture |
| Log masuk tetapi `unknown_pin` | PIN mesin ≠ `employee_number` (angka nol di depan, spasi) | tinjau log bermasalah di UI v2, samakan PIN lalu proses ulang |
| Log `period_closed` | bulan itu sudah ditutup di v2 | buka kembali periode lalu proses ulang |
| Waktu punch bergeser jam | zona mesin ≠ `devices.timezone` | samakan zona; log lama tidak berubah otomatis |
| `readyz` 503 | database mati, `DATABASE_URL` salah, hak role kurang, migration v2 belum diterapkan | log `pemeriksaan kesiapan database gagal`; bagian 3 |
| Template tidak tersimpan | `BIOMETRIC_KEY` belum diisi | peringatan saat start |
| Perintah `failed` | lihat `result_message` di UI v2 (PIN tak aman, versi algoritma beda, mesin menolak) | log `perintah ... gagal` |
