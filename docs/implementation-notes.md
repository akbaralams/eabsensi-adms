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
