# Task eabsensi-adms

Urutan wajib. Tiap task selesai bila kriteria "Selesai bila" terpenuhi dan sudah di-commit. Tandai `[x]` saat selesai.

## [x] Task 0. Toolchain
- Minta user memasang Rust stable terbaru (rustup, MSVC; butuh Visual Studio Build Tools "Desktop development with C++"). Jangan memasang sendiri tanpa izin.
- Pasang `sqlx-cli` terbaru (fitur postgres).
- **Selesai bila**: versi `cargo`, `rustc`, `sqlx` tercatat di README.

## [x] Task 1. Analisis acuan lama
Baca `../eabsensi-99` secara menyeluruh dan tulis `docs/legacy-analysis.md`:
- Setiap endpoint `iclock/*`: request yang dikirim mesin (query string, header `User-Agent`, body), balasan persis, dan efek ke database.
- Alur handshake (`cdata` GET dengan `options`), stamp (`Stamp`, `OpStamp`, `PhotoStamp`), opsi (`Delay`, `ErrorDelay`, `TransInterval`, `TransFlag`, `Realtime`, dst.), registrasi mesin baru (`DeviceTemp`, `registry`), antrean perintah (`CommandList`, `CommandsUser`, format `C:<id>:<perintah>`, hasil di `devicecmd`), unggah log (`ATTLOG`, `OPERLOG`, foto), `push`/`ping`/`rtdata`/`querydata`.
- Semua tabel dan kolom yang dipakai (dari model dan query), arti tiap kolom, dan relasinya ke pegawai.
- Fitur turunan: pemetaan pegawai ke mesin/area (`EmployeeDevice`, `EmployeeArea`), sinkron sidik jari (`EmployeeFP`), transaksi mencurigakan (`TransSuspects`) dan notifikasinya (`SendNotif`, `SendSuspectNotif`).
- Bug, celah keamanan, dan kebiasaan firmware yang ditangani kode lama.
- **Selesai bila**: dokumen lengkap untuk semua endpoint dan tabel, dengan contoh payload nyata; daftar fitur ditandai "pertahankan / buang / perlu keputusan".

## [x] Task 2. Usulan skema v2 dan keputusan fitur
- Tanyakan ke user setiap fitur berlabel "perlu keputusan" (mis. sinkron sidik jari dan data biometrik, transaksi mencurigakan dan notifikasinya, area/penugasan mesin, pendaftaran otomatis mesin baru, aturan punch ke bulan yang sudah ditutup, status punch mesin langsung `approved` atau tidak).
- Tulis `docs/schema-proposal.md`: tabel baru dengan nama netral snake_case, FK + index eksplisit, kunci unik idempotensi log, pemetaan PIN mesin ke `employees.employee_number`, kasus enum `PunchSource::Device`, dan data apa yang ditulis ke `attendance_punches`.
- Serahkan ke sesi `eabsensi-v2` untuk dibuat sebagai migration + UI kelola mesin (di repo itu, bukan di sini). Repo ini menunggu sampai migration ter-merge dan dump skema diperbarui.
- **Selesai bila**: keputusan user tercatat, usulan skema disetujui, dan `SCHEMA_VERSION` menunjuk dump yang sudah berisi tabel mesin.

## [x] Task 3. Kerangka service
- Crate biner `eabsensi-adms`: axum + tokio (versi terbaru), sqlx (postgres, offline), tracing log JSON, config dari env, `/healthz`, port sendiri (terpisah dari web dan API mobile), batas ukuran body, timeout.
- CI GitHub Actions: Postgres, muat dump skema, `cargo sqlx prepare --check`, fmt, clippy, test.
- **Selesai bila**: `cargo run` melayani `/healthz`; CI hijau.

## [x] Task 4. Handshake dan antrean perintah
- `GET /iclock/cdata` (opsi per mesin), `GET /iclock/getrequest` (kirim perintah tertunda), `POST /iclock/devicecmd` (catat hasil), heartbeat/`last_seen_at`, perlakuan mesin tak dikenal sesuai keputusan Task 2.
- **Selesai bila**: balasan byte-per-byte cocok dengan acuan lama untuk payload contoh dari Task 1 (test unit + integrasi).

## [x] Task 5. Penerimaan log
- `POST /iclock/cdata` `table=ATTLOG` (dan `OPERLOG` sesuai keputusan): parse, simpan log mentah idempoten, petakan PIN ke pegawai, tulis `attendance_punches` (`source=device`), simpan log yang PIN-nya tidak dikenal untuk ditinjau di web.
- Endpoint lain (`fdata`, `registry`, `push`, `ping`, `rtdata`, `querydata`) sesuai keputusan Task 2.
- **Selesai bila**: kiriman ulang batch yang sama tidak menambah punch; punch muncul di kehadiran harian web v2.

## [x] Task 5b. Biometrik terenkripsi (keputusan D1)
- Terima baris `FP`/`FACE`/`BIOPHOTO` (di `POST /iclock/cdata` `table=OPERLOG`, sementara `OPLOG` tetap diabaikan, D6) dan `table=tabledata` `templatev10`/`biophoto`; simpan ke `biometric_templates` hanya untuk pegawai dikenal, terenkripsi AES-256-GCM (kunci `BIOMETRIC_KEY` + `BIOMETRIC_KEY_ID`, AAD `employee_id|kind|finger_index|algorithm_version`). `face_photo` hanya bila `BIOMETRIC_STORE_FACE_PHOTO=true`.
- Dekripsi hanya di ADMS untuk merakit perintah `add_fingerprint` (`DATA FP PIN=..	FID=..	Size=..	Valid=1	TMP=..`); isi template tidak pernah ditulis ke log atau pesan galat.
- **Selesai bila**: uji unit enkripsi (bolak-balik, AAD salah ditolak, kunci lama dengan `key_id` lain), uji integrasi unggah lalu kirim ulang ke mesin lain, dan tidak ada isi template di log.

## [ ] Task 6. Simulator mesin
- Binary/test helper yang meniru mesin ZKTeco: handshake, kirim ATTLOG (termasuk kiriman ulang dan batch besar), polling perintah, kirim hasil perintah.
- **Selesai bila**: test integrasi end-to-end berjalan di CI dengan simulator; uji beban sederhana (mis. 100 mesin polling bersamaan) tercatat.

## [ ] Task 7. Operasional
- Panduan deploy (Windows service/systemd, port, firewall/VPN ke jaringan mesin), konfigurasi alamat server di mesin, pemantauan mesin offline.
- **Selesai bila**: README berisi langkah deploy dan pemeriksaan setelah deploy; diuji dengan minimal satu mesin fisik bila tersedia.
