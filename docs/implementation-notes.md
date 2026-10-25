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
