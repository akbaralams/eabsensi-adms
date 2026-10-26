# Catatan implementasi

Keputusan teknis yang diambil saat membangun, dan belum tertulis di [legacy-analysis.md](legacy-analysis.md) atau [schema-proposal.md](schema-proposal.md). Ditambah per task.

## Task 4: handshake, getrequest, devicecmd

### Cara pembuktian "byte-per-byte dengan acuan lama"
`tests/fixtures/generate_handshake.py` membaca pernyataan `$rsp = ...` / `$rsp .= ...` langsung dari `../eabsensi-99/.../IDeviceController.php` (rentang baris `iconnect`), mengevaluasinya, dan menulis byte hasilnya ke `tests/fixtures/handshake/*.txt` (tiga bentuk × dengan dan tanpa stamp). `tests/handshake_golden.rs` membandingkan keluaran Rust dengan berkas itu. Jadi kecocokan terbukti terhadap kode sumber lama, bukan terhadap salinan manual. `.gitattributes` mencegah Git mengubah akhir baris fixture.

Yang **belum** terbukti: kecocokan dengan firmware sungguhan. Fixture berasal dari kode lama; isi body request asli masih harus ditangkap (legacy-analysis bagian 10), lewat simulator (Task 6) atau mesin fisik.

### Mesin tak terdaftar atau nonaktif (D2)
Tidak ada efek di database (tanpa heartbeat, tanpa perintah) dan hanya dicatat di log (`warn`, nomor seri saja). Balasannya meniru kode lama supaya firmware berperilaku sama:

| Endpoint | Balasan untuk mesin ditolak, request cacat, atau `User-Agent` bukan `iClock` |
|---|---|
| `GET /iclock/cdata` | 200, body kosong, `text/html; charset=UTF-8` (kode lama mengembalikan `null`) |
| `GET /iclock/getrequest` | 200, `OK` |
| `POST /iclock/devicecmd` | 200, `OK\n` |

### Pemilihan bentuk balasan handshake
Percabangan `iconnect` lama dipetakan ke skema baru: `is_new` menjadi `devices.initialized_at IS NULL`.

| Keadaan mesin | Bentuk |
|---|---|
| Belum terinisialisasi, request `DeviceType=att` atau `0` | `Att` (dan `devices.device_type` menjadi `att`) |
| Belum terinisialisasi, selain itu | `Basic` |
| Terinisialisasi dan `push_version` terisi | `Att` bila `device_type = att`, selain itu `Push23` |
| Terinisialisasi tanpa `push_version` | `Basic` |
| `pushver` diawali `3.` dan `PushOptionsFlag=1` | `OK\nPushProtVer=<pushver>\nPushOptionsFlag=1` (tanpa LF akhir) |

Mesin yang belum terinisialisasi diantrekan perintah `info` pada **setiap** handshake sampai INFO berhasil (index unik parsial mencegah ganda). INFO sukses menandai `initialized_at` dan mengantrekan `check`; INFO gagal tidak menandainya (kode lama menandai `is_new = 0` apa pun hasilnya).

`TimeZone=` berasal dari `devices.timezone` (IANA) dihitung ke jam pada saat request (`7`, `5.5`, `5.75`); `ErrorDelay`/`Delay` dari `error_delay_seconds`/`poll_delay_seconds`.

### Antrean perintah
- Kelas prioritas dan batas per poll sama dengan kode lama: 1 sistem (1), 2 pengaturan (10), 3 user (50), 4 jari/wajah (5). Hanya kelas terkecil yang punya perintah menunggu yang dikirim pada satu poll. Pengambilan memakai `FOR UPDATE SKIP LOCKED`.
- `sent` tanpa jawaban lebih lama dari `ADMS_COMMAND_ACK_TIMEOUT_SECS` (120) kembali `pending`; setelah `ADMS_COMMAND_MAX_ATTEMPTS` (3) kali dikirim menjadi `failed` ("tidak ada jawaban dari mesin").
- Jawaban mesin hanya diterima untuk perintah milik mesin itu dan yang belum selesai (`sent` atau `pending`); id yang sama dari mesin lain, atau jawaban kedua, diabaikan. `Return=0` = `succeeded`, selain itu `failed` dengan `return_code` (kode lama menandai kegagalan `DATA`/`REBOOT` sebagai sukses).
- `available_at` bertipe `timestamptz(0)` sehingga nilai bawaan `now()` dibulatkan dan bisa tampak sampai 0,5 detik di depan; klaim memakai toleransi 1 detik. (Ditemukan lewat uji yang kadang gagal.)

### Bentuk baris perintah
| Jenis | Baris yang dikirim |
|---|---|
| `info`, `check`, `reboot` | `C:<id>:INFO`, `C:<id>:CHECK`, `C:<id>:REBOOT` |
| `set_time` | **dua baris dengan id yang sama**: `C:<id>:SET OPTIONS MachineTZ=±HHMM` lalu `C:<id>:SET OPTIONS DateTime=<detik ZK UTC>` |
| `add_user` | `C:<id>:DATA USER PIN=<employee_number>\tName=<nama>\tPri=0` (nama dibersihkan dari tab/LF; PIN hanya huruf, angka, `_`, `-`) |
| `add_fingerprint` | belum didukung: ditandai `failed` ("jenis perintah ini belum didukung server") sampai biometrik dikerjakan |

Catatan `set_time`: kode lama membuat dua perintah terpisah (id berbeda); skema baru punya satu jenis, jadi satu id dirender menjadi dua baris. Mesin akan menjawab dua kali dengan id sama; jawaban pertama yang menentukan hasil. **Belum diuji pada firmware sungguhan.** Nilai `DateTime` adalah detik ZK dari waktu UTC (sama dengan `jamput()` lama bila `TIMEZONE` server = 7).

`SET ABCPIN` (`SET OPTION IsSupportABCPin=1`, dibuat otomatis oleh kode lama untuk mesin baru) **tidak dibawa**: konfigurasi v2 membatasi PIN ke angka, sehingga mode PIN alfanumerik tidak diperlukan. Bila nanti PIN huruf dipakai, tambahkan sebagai jenis perintah.

### Pengujian
Role Postgres dev (`absensi_dev`) tidak punya `CREATEDB`, jadi `#[sqlx::test]` tidak dipakai. Uji integrasi (`tests/iclock.rs`) membuat mesin dan pegawai bernama unik (`TEST-ADMS-…`, departemen "Departemen uji ADMS") di database yang sama dan menghapusnya sendiri; sisa uji yang gagal disapu bila lebih tua dari satu jam. Pool tiap uji dibatasi 2 koneksi agar uji paralel tidak menghabiskan `max_connections`.

## Task 5: penerimaan log dan endpoint pendamping

### Alur `POST /iclock/cdata`
Jenis unggahan dipilah menurut `table` dengan urutan `idata` lama (`protocol::attlog::classify`). Hanya `ATTLOG` (atau `Stamp` tanpa `table`) yang menghasilkan data:

| Unggahan | Efek | Balasan |
|---|---|---|
| `ATTLOG` / `Stamp` | log mentah + punch (di bawah); `attlog_stamp` = `Stamp` | `OK:<n>\n` |
| `OPERLOG`/`USERINFO`/`FINGERTMP`/`OpStamp` | hanya `operlog_stamp` = `OpStamp` (OPLOG diabaikan, D6; FP/FACE menunggu Task 5b) | `OK:<n>\n` |
| `BIODATA` | hanya `biodata_stamp` = `Stamp` | `OK\n` |
| `ATTPHOTO` / `PhotoStamp` | hanya `photo_stamp` (foto diabaikan, D7) | `OK\n` |
| `rtstate`, `rtlog` | tidak ada (D7) | `OK` |
| `options` | info perangkat (badan dipisah koma) | `OK` |
| `tabledata` | tidak ada (biometrik menyusul di Task 5b) | `OK\n` |
| selain itu | tidak ada | `OK\n` |

`<n>` = jumlah baris tak kosong di badan, **termasuk baris rusak** (agar mesin tidak macet mengirim ulang baris yang tak pernah bisa diproses). Kode lama menghitung juga elemen kosong di ujung badan (selisih satu); mesin tidak bergantung pada angka itu sejauh diketahui, belum dibuktikan pada firmware.

Stamp yang dikembalikan di handshake adalah **stamp yang dikirim mesin** (hanya angka, maksimal 20 digit; selain itu diabaikan), disimpan pada transaksi yang sama dengan datanya sehingga tidak pernah maju mendahului data. Apakah ini menghentikan pengiriman ulang batch yang sama oleh firmware masih hipotesis (legacy-analysis 2.2); uji dengan simulator/mesin.

### Penerimaan satu batch ATTLOG (`ingest::ingest_attlog`)
Satu transaksi per batch; galat database membatalkan semuanya dan dijawab 500 (mesin tidak mendapat `OK`, jadi mengirim ulang):

1. Penguraian murni (`parse_attlog`): `PIN \t waktu \t status \t verifikasi \t ...`; PIN dipakai apa adanya (maks 32 karakter, tanpa trim dan padding, D9); baris rusak (tanpa waktu, PIN kosong/terlalu panjang, waktu tak terbaca, atau waktu di celah DST) dilewati dan dicatat tanpa isi baris.
2. Waktu lokal mesin → UTC memakai `devices.timezone`.
3. **Penyaringan kiriman ulang dengan satu query** (`UNNEST` terhadap kunci `(device_id, pin, device_time)`), sehingga batch lama yang dikirim ulang tiap koneksi tidak memicu query per baris. `INSERT ... ON CONFLICT DO NOTHING` tetap menjaga kiriman bersamaan dan baris kembar di batch yang sama.
4. Pegawai (cocok persis `employee_number`) dan periode terkunci diambil sekali per batch; keputusan outcome memakai fungsi murni `ingest::outcome::evaluate` (urutan: `out_of_range`, `unknown_pin`, `employee_inactive`, `period_closed`, `duplicate`, `punched`). Hanya pemeriksaan duplikat yang butuh query per baris yang lolos sampai tahap itu.
5. Log mentah disimpan dengan outcome-nya (PIN tak dikenal ikut disimpan, `employee_id` kosong); bila `punched`, punch (`source=device`, `status=approved`, kolom lain kosong) dibuat dan ditautkan lewat `punch_id`. `is_suspect` = `punched_at` lebih dari `ADMS_SUSPECT_FUTURE_SECS` (500) di depan waktu terima; tidak memengaruhi outcome (D4b).
6. `source_ip` = alamat soket klien (belakang proxy akan berisi alamat proxy; `X-Forwarded-For` belum dipakai).

Hasil ukur (uji `batch_besar...`, Postgres lokal): 2.000 baris baru ≈ 2,4 detik dalam satu transaksi; kiriman ulang batch yang sama ≈ 36 ms. Belum ada batas jumlah baris per request selain batas ukuran badan (`ADMS_MAX_BODY_BYTES`) dan timeout 30 detik.

### Kasus uji bersama dengan v2
`tests/fixtures/outcome-cases.json` adalah salinan **persis** dari `eabsensi-v2/tests/contract/adms/outcome-cases.json` (18 kasus); `tests/outcome_cases.rs` menjalankannya terhadap `evaluate`. Ubah hanya lewat PR pasangan di kedua repo. Uji integrasi `tests/ingest.rs` memeriksa hal yang sama lewat database (outcome, penautan pegawai/punch, periode `closed`/`closing`/`open`).

### Mesin tak terdaftar atau nonaktif pada unggahan
Dijawab **kosong** (bukan `OK`) tanpa efek apa pun. Alasannya: balasan `OK` membuat mesin menganggap datanya terkirim dan membuangnya, sehingga absensi mesin yang baru didaftarkan operator akan hilang. Dengan balasan kosong mesin menyimpan data dan mengirim ulang setelah terdaftar. (Kode lama membalas `OK:n` dan membuang datanya.) **Asumsi perilaku firmware belum diuji**: bila firmware ternyata membuang data pada balasan non-OK atau berhenti mencoba, ubah menjadi `OK` dan simpan log mentahnya.

### Endpoint pendamping
| Endpoint | Perilaku |
|---|---|
| `POST /iclock/fdata` | foto diabaikan (D7), `OK\n` |
| `POST /iclock/registry` | mesin tak dikenal: `UNKNOWN DEVICE`; dikenal: info diperbarui, `DeviceType` 1 → `acc`, `att`/`0` → `att`, `RegistryCode=<10 karakter>` dari CSPRNG dan tetap setelah dibuat |
| `POST /iclock/push` | parameter sesi (`ServerVersion` dari `push_version`, bawaan `2.3.0`) + mengantrekan `set_time`; tanpa LF akhir |
| `GET /iclock/rtdata?type=time` | `DateTime=<detik ZK UTC>,ServerTZ=<±HHMM zona mesin>` |
| `GET /iclock/ping` | `OK`, tanpa pemeriksaan |
| `POST /iclock/querydata` | A1: `options` → `OK`; `count` memperbarui jumlah user/jari/wajah; `tabledata` memantulkan `<tablename>=<count>` (hanya karakter aman) tanpa menyimpan data |

### Catatan pengujian
Tanggal uji 2024-06 dan 2022 diasumsikan tidak berada di periode terkunci pada database uji; uji periode memakai Maret-Mei 2021 dan menghapusnya sendiri. Efek ke kehadiran harian v2 dibaca dari kode (kalkulator v2 hanya menyaring `status = approved`, bukan `source`), belum dilihat di UI v2.

## Task 5b: biometrik terenkripsi (D1)

### Konfigurasi
| Variabel | Arti |
|---|---|
| `BIOMETRIC_KEY` | Kunci AES-256 dalam base64 (tepat 32 byte). **Tanpa kunci ini template tidak diterima dan tidak bisa dikirim**; ADMS tidak pernah menyimpan template tanpa enkripsi. Saat start ada peringatan di log. |
| `BIOMETRIC_KEY_ID` | Id kunci terkini (1-16 karakter huruf, angka, `_`, `-`); wajib bila `BIOMETRIC_KEY` diisi. Disimpan di setiap baris (`key_id`). |
| `BIOMETRIC_OLD_KEYS` | `id=base64,id=base64`: kunci lama yang hanya dipakai untuk membaca setelah rotasi. |
| `BIOMETRIC_STORE_FACE_PHOTO` | `true` agar foto wajah (`BIOPHOTO`) ikut disimpan; bawaan `false` (tafsir A6/Q8). |

Nilai kunci tidak pernah ikut pesan galat konfigurasi maupun `Debug`; kunci disimpan di memori sebagai `Zeroizing` dan `aes-gcm` dibangun dengan fitur `zeroize`.

### Enkripsi (`src/vault.rs`)
AES-256-GCM, nonce 12 byte acak dari CSPRNG sistem untuk tiap enkripsi, tag 16 byte. **AAD** = `employee_id|kind|finger_index|algorithm_version`, jadi ciphertext tidak dapat dipindah ke pegawai, jenis, jari, atau versi lain. Galat dekripsi sengaja tanpa rincian (kunci salah, AAD berbeda, dan data rusak tidak dibedakan). Rotasi: kunci baru dipakai untuk semua enkripsi; baris lama dibuka dengan kunci lamanya dan **dienkripsi ulang ke kunci terkini saat batch yang sama dikirim ulang** oleh mesin.

### Penerimaan
- Baris dari `POST /iclock/cdata`: `FP `/`FACE `/`BIOPHOTO ` di badan `table=OPERLOG`, serta `table=tabledata` dengan `tablename=templatev10`/`biophoto`. `OPLOG` dan `USER` tetap diabaikan (D6). `BIODATA` (firmware generasi baru) tetap hanya menyimpan stamp, seperti kode lama: template dari firmware itu **belum** diterima.
- Hanya pegawai yang cocok persis `employee_number` yang boleh memiliki template; PIN lain dihitung dan dibuang. Satu baris per `(pegawai, jenis, jari, versi algoritma)` (`ON CONFLICT ... DO UPDATE`).
- **Versi algoritma** diambil dari laporan mesin di `devices.reported_options` (`~ZKFPVersion`, `FaceVersion`; kosong bila belum dilaporkan). Template dari mesin versi berbeda tidak saling menimpa.
- Template yang sama persis (dan sudah memakai kunci terkini) tidak ditulis ulang (mesin mengirim ulang batch tiap koneksi); pembandingan dilakukan setelah dekripsi di memori.
- Satu transaksi per unggahan, stamp `OpStamp` disimpan di transaksi yang sama. Batas: 64 KiB per template, 512 KiB per foto, PIN maksimal 32 karakter; baris di luar itu dilewati dan hanya dihitung.
- Tanpa brankas: isi dibuang, hanya stamp yang disimpan, balasan tetap `OK:<n>`.
- Bila `BIOMETRIC_KEY` baru diisi setelah mesin terlanjur mengirim batch, template tidak masuk sampai mesin mengirim ulang batch itu (stamp OPERLOG tetap maju); tangkap lewat perintah/penarikan ulang atau daftarkan ulang di mesin.

### Pengiriman ke mesin lain (`add_fingerprint`)
Perintah v2 merujuk `employee_id` dan `biometric_template_id`. Saat `getrequest` ADMS memeriksa: brankas aktif, template ada, milik pegawai pada perintah, berjenis sidik jari, `is_valid`, dan versi algoritma cocok dengan `~ZKFPVersion` mesin tujuan (bila keduanya diketahui). Lolos: template didekripsi **hanya di titik ini** dan dikirim sebagai
`C:<id>:DATA FP PIN=<employee_number>	FID=<nomor jari>	Size=<ukuran asli>	Valid=1	TMP=<base64>` (format kode lama). Gagal: perintah `failed` dengan alasan singkat yang aman ditampilkan di UI v2 (tanpa isi template atau rincian kunci). Hanya jenis `fingerprint` yang bisa dikirim; template wajah dan foto tersimpan tetapi belum bisa didorong ke mesin (format perintahnya belum diketahui dari acuan lama).

### Tanpa isi di log
`Debug` untuk template mentah, ciphertext, dan brankas hanya menampilkan panjang atau id kunci. Log penerimaan hanya memuat jumlah. Uji `tests/biometric_logs.rs` menangkap seluruh keluaran `tracing` (termasuk pernyataan SQL sqlx, yang hanya berisi placeholder) selama unggah, kirim ke mesin lain, dan jalur gagal (kunci salah), lalu memastikan base64/teks template, baris rusak, dan kunci tidak muncul. Uji itu sengaja di berkasnya sendiri karena penangkap log memakai subscriber thread-local yang terganggu uji paralel lain di proses yang sama.

### Temuan saat menguji
Pemangkasan spasi sebelum mengganti spasi menjadi `+` merusak template yang diawali atau diakhiri `+` (mesin mengirim `+` sebagai spasi). Diperbaiki dan diuji (`spasi_di_awal_dan_akhir_nilai_juga_tanda_plus`).

## Task 6: simulator mesin

### Apa yang ditiru
- **Klien HTTP mentah** (`simulator::http`): HTTP/1.1, satu koneksi per request, `Connection: close`, header `Content-Type:` kosong, `Host: <ip>:<port>`, `User-Agent: iClock Proxy/1.09`, urutan header seperti log produksi. Dibuat sendiri (bukan `reqwest`) agar bentuk request persis sama dan tanpa dependensi baru.
- **Mesin** (`simulator::Machine`): ingatan log, daftar user, template, opsi yang diterima. Siklus `cycle()` = handshake, unggah log, polling, jalankan perintah (INFO dijawab dengan badan info perangkat; `CHECK`, `REBOOT`, `SET OPTIONS`, `DATA USER`, `DATA FP` dijawab per `ID`; `DATA FP` ditolak bila `Size` tidak cocok dengan panjang template, seperti firmware yang memeriksa panjangnya). Mesin yang ditolak server berhenti setelah handshake dan datanya tetap di ingatan.
- **Kebijakan kirim ulang** (`Resend`): `All` mengirim seluruh ingatan tiap koneksi (yang tercatat di log lama); `FromStamp` hanya log yang detik ZK-nya melewati stamp yang dikembalikan server (hipotesis bila stamp bekerja). Kedua jalur diuji end-to-end.
- **`Stamp` di unggahan** = detik ZK log terakhir di batch itu (sama dengan nilai nyata di log produksi, mis. `785168153`).

### Uji end-to-end (`tests/e2e.rs`, server dan simulator lewat TCP)
Siklus mesin baru (INFO, CHECK, bentuk handshake berubah, kiriman ulang berkali-kali tanpa duplikat), jalur `FromStamp`, batch 5.000 baris dalam 5 request, dua koneksi bersamaan dari mesin yang sama (tidak ada punch ganda), perintah operator (reboot, set_time dua baris, add_user) dijalankan dan hasilnya tercatat, mesin tak terdaftar/nonaktif **tidak kehilangan data** dan seluruh backlog masuk setelah didaftarkan (keputusan balasan kosong, bukan `OK`), serta template sidik jari dari satu mesin sampai utuh ke mesin lain lewat server.

### Uji beban
Lihat [load-test.md](load-test.md): 100 mesin bersamaan, 4.700 request siklus penuh dan 3.000 polling tanpa galat, ±425-460 request/detik pada laptop pengembangan, p99 < 1 detik.

### Catatan teknis
- `cargo run` memakai `default-run = "eabsensi-adms"` (ada dua biner: server dan `adms-sim`).
- `Machine::take_stats()` menyerahkan latensi dan galat per request untuk dirangkum `LatencyStats` (persentil nearest-rank).
- Penyapu sisa uji (`pool()` di `tests/common`) kini menghapus log mentah mesin uji yang sudah tua sebelum menghapus mesinnya (`device_logs.device_id` bersifat RESTRICT); sebelumnya satu sisa uji bisa membuat semua uji berikutnya gagal.
- Dalam e2e, kelas prioritas dikirim satu per koneksi, jadi `CHECK` (kelas 1) mendahului `add_fingerprint` (kelas 4); uji menjalankan beberapa siklus.

## Task 7: operasional

- **Subperintah** (`src/cli.rs`): `check-offline` dan `rekey-biometrics`; tanpa argumen atau `serve` menjalankan server. Penguraian argumen manual (tanpa `clap`) dan diuji. Perintah operasional memakai kolam 2 koneksi yang terhubung segera (galat koneksi langsung terlihat), bukan kolam malas server.
- **`check-offline`** (`src/offline.rs`): mesin aktif yang `last_seen_at`-nya lebih lama dari `max(faktor x poll_delay_seconds, minimum)` (bawaan 3 dan 120 detik) atau belum pernah terhubung. UI v2 memakai 3 x jeda tanpa minimum; minimum 120 detik di sini mencegah peringatan berkedip. Status keluar 0/2/1 agar bisa dipakai penjadwal.
- **`rekey-biometrics`** (`src/rekey.rs`): membuka tiap template dengan kunci lamanya dan mengenkripsi ulang ke kunci terkini, per 200 baris dalam satu transaksi, aman diulang dan aman terhenti di tengah. `UPDATE ... WHERE key_id = <lama>` sehingga baris yang ditimpa mesin selama proses tidak tertimpa balik. Baris yang tidak bisa dibuka dilaporkan (hanya id) dan tidak diubah. `--from` membatasi ke satu `key_id`, `--dry-run` hanya memastikan semua baris bisa dibuka. Ini melengkapi rotasi pasif (baris berpindah kunci hanya saat mesin mengirim ulang) sehingga kunci lama bisa dipastikan aman dibuang.
- **Hak database minimal** diturunkan dari seluruh query `sqlx` di `.sqlx/` (pemindai sederhana atas berkas metadata): `attendance_punches` SELECT/INSERT; `biometric_templates`, `device_commands`, `device_logs` SELECT/INSERT/UPDATE; `devices` SELECT/UPDATE; `employees`, `periods` SELECT; tanpa `DELETE`, ditambah USAGE urutan id empat tabel yang di-INSERT. Belum diuji dengan role terpisah.
- **Build rilis**: `SQLX_OFFLINE=true cargo build --release --locked` ±1 menit di mesin pengembangan; `eabsensi-adms` ±7 MB, `adms-sim` ±0,8 MB.
- **Yang tidak dikerjakan dan alasannya**: pembungkus service Windows (butuh perangkat pihak ketiga yang perlu disetujui; contoh NSSM ada tetapi belum diuji), Dockerfile (tidak diminta), metrik Prometheus (pemantauan memakai `check-offline`, `/healthz`, `/readyz`, dan log), dukungan `X-Forwarded-For` (tercatat sebagai batas di operations.md), mode perekaman payload di server (packet capture cukup dan tidak menambah risiko data pribadi di log).
- **Uji mesin fisik** tidak mungkin saat pengembangan; [field-test-checklist.md](field-test-checklist.md) memuat 13 pengamatan terhadap hipotesis yang masih terbuka beserta tindakan bila hasilnya lain.

## Temuan uji mesin fisik pertama (26 Oktober 2026)

Mesin: model `UA200`, platform `ZEM560_TFT`, firmware `Ver 6.60 Jul 24 2013`, serial yang sama dengan yang muncul di log produksi lama. Terhubung ke server pengembangan lewat jaringan lokal.

Yang terbukti:
- Handshake bentuk dasar (firmware lama) diterima: mesin terus polling, menjawab `INFO` dan `CHECK`, dan `REBOOT` dua kali dijawab `Return=0`.
- Penerimaan ATTLOG nyata: 58 log dari 5 PIN, semuanya `unknown_pin` (belum ada pegawai), tanpa baris rusak. Delapan menit kemudian seluruh 58 log dikirim ulang (`kiriman_ulang = 58`, tidak ada log ganda): mesin mengirim ulang seluruh ingatannya pada koneksi berikutnya, persis perilaku yang tercatat di log produksi lama. Pemicunya **belum diketahui**; semula diduga reboot, tetapi operator melaporkan mesin tidak restart saat perintah `REBOOT` dikirim, jadi kaitannya tidak terbukti.
- **`REBOOT` dijawab `Return=0` tetapi layar mesin tidak restart** (diamati operator). Server mengirim `C:<id>:REBOOT` persis seperti kode lama dan mesin menjawab dalam detik yang sama, jadi saluran perintah bekerja (`CHECK` memicu unggahan log, `INFO` dijawab lengkap). Firmware 6.60 (2013) tampaknya mengakui perintah itu tanpa melaksanakannya lewat jalur ini. Belum diteliti lebih jauh; fitur reboot dari operator tidak bisa diandalkan untuk model ini. Status `succeeded` di UI v2 hanya berarti mesin membalas OK.
- `INFO` firmware ini **tidak memuat `PushVersion`**, jadi `devices.push_version` tetap kosong dan handshake memakai bentuk dasar.

Yang tidak seperti hipotesis:
- **`set_time` tidak dijawab** mesin (tiga kali dikirim, lalu `failed`: "tidak ada jawaban dari mesin"), sementara `REBOOT` dijawab. Firmware ini tidak menjawab `SET OPTIONS MachineTZ=...` / `SET OPTIONS DateTime=...` (dua baris dengan id sama). Penyebab belum diketahui (nama perintah, jumlah baris, atau firmware yang tidak mendukung). Kode lama tidak pernah mengirim perintah ini ke mesin yang tidak memanggil `push`.
- **Jam mesin tidak ikut tersinkron** seperti pada server lama. Hipotesis utama: firmware menyelaraskan jam dari header `Date` respons (server lama tidak mengirim perintah jam ke mesin ini, dan log lama tidak memuat `push`/`rtdata`). Respons hyper memakai nama header huruf kecil (`date:`, `content-type:`), sedangkan Apache/PHP mengirim `Date:`, `Content-Type:`. Pengurai firmware 2013 yang peka huruf besar/kecil tidak akan menemukan `date:`.
  - Perbaikan yang dicoba: penyaji sendiri di atas hyper (`src/http/serve.rs`) dengan `title_case_headers(true)`, sehingga respons memakai `Date:`, `Content-Type:`, `Content-Length:`, `Connection:` seperti server lama. Diuji di `tests/wire.rs` (nama header kapital, `Date` berformat RFC 2822 dan akurat, berhenti rapi).
  - **Belum terbukti** menyelaraskan jam mesin; perlu dicoba pada mesin ini. Bila tidak berhasil: periksa isi header respons dengan Wireshark, lalu coba bentuk perintah `SET OPTION DateTime=...` (tunggal), dan bandingkan dengan tangkapan lalu lintas ke server lama bila masih tersedia.

Perubahan kode dari temuan ini: penyaji dengan header kapital awal, log diagnostik (`perintah dikirim ke mesin`, `hasil perintah dari mesin`, nilai `Stamp` mentah) pada level debug/info tanpa isi perintah (nama pegawai dan template tidak pernah dicatat).

### Lanjutan: log debug mesin yang sama (26 Oktober 2026, ±2 jam polling)

Dibaca dari log server dengan `RUST_LOG=eabsensi_adms=debug,tower_http=debug,info`. Stempel ZK di URI didekode dengan `zk_time` (bulan 31 hari); semua waktu mesin di bawah adalah waktu lokal WIB.

- **Stamp ternyata dikirim, dan pengembalian stamp oleh server menghentikan kiriman ulang.** Unggahan ATTLOG membawa `Stamp=<detik ZK>` (mis. `862000778` = 2026-10-26 20:39:38). Pada unggahan pertama setelah restart server belum punya stamp, jadi mesin mengirim seluruh ingatan (`baris 59, kiriman_ulang 58, punch 1`). Setelah server menyimpan stamp itu dan mengembalikannya di handshake, unggahan berikutnya hanya membawa log baru (`baris 1, kiriman_ulang 0, punch 1`). Ini bukti pada mesin fisik untuk hipotesis `Resend::FromStamp`. Hal yang sama terlihat pada `OpStamp` (OPERLOG empat batch saat sambungan pertama, satu batch pada sambungan berikutnya). Catatan sebelumnya bahwa mesin tidak mengirim `Stamp` keliru: unggahan awal 58 log memang tidak terlihat membawa stamp yang tersimpan, tetapi unggahan berikutnya membawanya.
- **Absensi end-to-end bekerja.** Dua absensi nyata pegawai 181164 menjadi `punched` (`punch: 1` pada 13:39:40Z dan 13:55:16Z).
- **Jam mesin tersinkron oleh server (terbukti).** Sebelum penyaji header kapital dipasang operator melaporkan jam mesin tidak ikut tersinkron seperti pada server lama; sesudahnya jam mesin yang diubah manual **kembali benar sendiri** dalam 1-2 menit tanpa perintah operator, dan stamp mesin sendiri didekode menjadi 20:39:38 dan 20:55:15 WIB, cocok dengan jam sebenarnya (20:39:40 dan 20:55:16, selisih = waktu server menerima). Hipotesis header `Date` terbukti pada mesin ini: firmware 6.60 menyelaraskan jam dari header `Date` respons, dan header huruf kecil bawaan hyper (`date:`) tidak dikenalinya. Yang belum bisa dipisahkan: apakah hanya `Date:` yang berpengaruh atau seluruh header berhuruf kapital (nama header lain ikut diubah bersamaan). Perintah `set_time` karena itu tidak diperlukan untuk model ini.
  - Petunjuk tambahan: OPERLOG berisi batch bercap waktu `2000-10-20 21:16:28` dan `1970-01-01 00:00:00` (tepat nol epoch). Jam mesin ini pernah kembali ke 1970/2000, lazim bila baterai RTC lemah dan jam hilang saat listrik padam (inferensi). Karena sinkronisasi sekarang bekerja, mesin akan diperbaiki sendiri setiap menyala.
- **`REBOOT` tidak melakukan reboot.** Setelah perintah #1993 dikirim (14:06:48) mesin membalas `Return=0` dalam 0,2 detik lalu polling lagi 1,6 detik kemudian, dan tidak ada handshake baru. Reboot sungguhan akan senyap sekitar setengah menit sampai satu menit lalu diikuti handshake `cdata` (seperti setelah `CHECK`, yang memicu handshake dan unggahan segera). Kesimpulan: firmware mengakui `REBOOT` tetapi mengabaikannya. Kiriman ulang 58 log pada 13:20:43 tidak berkaitan dengan reboot.
- **Perbandingan `REBOOT` dengan server lama** (operator melaporkan di server lama mesin yang sama bisa direstart): di `eabsensi` dan `eabsensi-99` reboot diantrekan lewat `commands_user` (`SetStatusDevice` saat mesin diaktifkan/dinonaktifkan) atau `command_list` (`REBOOT DEVICE`, data `REBOOT`, kelas 1), dan dikirim sebagai `C:<id>:REBOOT\n`, **byte-per-byte sama** dengan yang dikirim server ini (diuji di `perintah_sederhana_cocok_dengan_log_produksi`). Hasil mesin dibaca dengan cara yang sama (`CMD=REBOOT`). Jadi perbedaannya bukan pada teks perintah. Yang belum bisa dibedakan dari kode: alur lama yang lain (mis. `SET OPTION IsSupportABCPin=1` yang di server lama otomatis disusul `REBOOT`, dan `CLEAR DATA` + `REBOOT`) memicu restart, atau ada perbedaan di tingkat HTTP selain nama header. Log lama tidak memuat satu pun `REBOOT`.
- **Pola polling**: interval antar `getrequest` mulai ±1,6 detik setelah suatu kejadian (perintah, unggahan, sambungan ulang) lalu naik sekitar 3 detik per polling sampai mencapai ±60 detik (`Delay`). Perilaku firmware yang normal, bukan galat.
- **`USERINFO` dan `FINGERTMP`**: setelah pegawai dibuat/diubah di mesin, mesin mengunggah `table=USERINFO` lalu enam kali `table=FINGERTMP` (jumlah sidik jari 6). Tanpa `BIOMETRIC_KEY` template dibuang; hanya stamp (`OpStamp=862000843`) yang disimpan.

