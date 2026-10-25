# Brief serah-terima untuk sesi `eabsensi-v2`

Untuk disalin ke sesi Claude Code dengan **folder aktif `C:\laragon\www\eabsensi-v2`**. Brief ini berdiri sendiri; sumber kebenaran rinci ada di repo `eabsensi-adms`:

- `../eabsensi-adms/docs/schema-proposal.md` (DDL, aturan, keputusan D1-D11, default A1-A6)
- `../eabsensi-adms/docs/legacy-analysis.md` (perilaku protokol lama; hanya perlu dibaca bagian 4 dan 5 untuk konteks perintah)

Status persetujuan (25 Oktober 2026): usulan skema **disetujui user**, termasuk default A1-A6 dan usulan Q1-Q8 (rincian di bagian 6).

---

## Prompt untuk sesi eabsensi-v2 (salin dari sini)

> Kamu mengerjakan `eabsensi-v2` (Laravel 13, PHP 8.5, PostgreSQL). Baca `CLAUDE.md` repo ini dan `docs/rust-services-plan.md` bagian "Batas dengan ADMS" lebih dulu. Lalu baca `../eabsensi-adms/docs/schema-proposal.md` penuh (terutama bagian 2, 3, 4, 8). Tugasmu: menyiapkan sisi v2 untuk server ADMS (mesin absensi ZKTeco) yang ditulis di repo Rust terpisah `eabsensi-adms`. **Repo ini pemilik skema**; kamu membuat migration, enum, UI kelola mesin, dan memperbarui dump skema. Kamu **tidak** menulis kode protokol mesin dan tidak mengubah repo lain.
>
> Kerjakan berurutan, satu commit per langkah (Conventional Commits), dan jalankan `./vendor/bin/pint`, `php artisan test` (Pest, termasuk `ArchTest`) sebelum tiap commit. Ikuti standar kode di `CLAUDE.md` (strict_types, `final`, Repository Pattern, backed enum, tanpa `env()` di luar `config/`).

### Langkah 1. Enum `PunchSource::Device`
- Tambah `case Device = 'device'` di `app/Enums/PunchSource.php`, label "Mesin absensi". Kolom `attendance_punches.source` sudah `varchar(16)`, jadi **tanpa migration**.
- Cari semua `match`/pemakaian enum ini (UI, repository, resource, laporan) dan pastikan nilai `device` tampil benar dan tidak memicu error `UnhandledMatchError`. Tambah test.
- Punch bersumber `device` tidak punya foto/lokasi; UI daftar punch harus tahan nilai `NULL` itu.

### Langkah 2. Migration (urutan wajib karena FK)
Buat empat migration baru, kompatibel mundur (hanya penambahan):
1. `create_devices_table`
2. `create_biometric_templates_table`
3. `create_device_commands_table` (butuh `biometric_templates`)
4. `create_device_logs_table` (butuh `devices`, `employees`, `attendance_punches`)

Sumber kolom, tipe, FK, dan index: `schema-proposal.md` bagian 3.1-3.4. Aturan:
- Nama snake_case tanpa prefix; **FK dan index eksplisit** di semua FK dan kolom filter laporan; perilaku `ON DELETE` persis seperti usulan (`RESTRICT` untuk `device_logs.device_id`, `CASCADE` untuk perintah dan template pegawai, `SET NULL` untuk sisanya).
- Index parsial (`device_logs_punch_id_unique`, `device_logs_review_index`, `device_commands_active_maintenance_unique`) lewat `DB::statement`. **Test suite memakai SQLite in-memory**: pastikan migration tetap jalan di SQLite (sintaks `WHERE` pada index didukung) atau lindungi dengan pemeriksaan driver; jangan merusak `php artisan test`.
- Kolom `jsonb`/`bytea`/`timestamptz`: pakai tipe Blueprint (`jsonb`, `binary`, `timestampTz`); periksa hasil SQL di Postgres, bukan hanya SQLite.
- Model `final`, di-cast ke enum, relasi lengkap, `visibleTo($user)` pada `Device` berdasarkan `region_id` mengikuti `DataScope`.
- `biometric_templates`: model **tanpa** accessor yang mendekripsi. Kolom `ciphertext`, `nonce` dihilangkan dari `$visible`/serialisasi (`$hidden`) agar tidak bocor lewat Inertia/JSON. V2 tidak pernah punya kunci enkripsi.

### Langkah 3. Enum baru (`app/Enums`)
`DeviceType` (`att`, `acc`, `unknown`), `DeviceCommandType` (`info`, `check`, `reboot`, `set_time`, `add_user`, `add_fingerprint`; metode `priority()` memetakan ke 1/2/3/4 sesuai tabel kelas di proposal), `DeviceCommandStatus` (`pending`, `sent`, `succeeded`, `failed`, `cancelled`), `DeviceLogOutcome` (`punched`, `unknown_pin`, `employee_inactive`, `period_closed`, `out_of_range`, `duplicate`; metode `isReprocessable()`), `BiometricKind` (`fingerprint`, `face`, `face_photo`). Label bahasa Indonesia.

### Langkah 4. Repository, Action, UI kelola mesin
Mengikuti pola v2 (kontrak di `Repositories/Contracts`, implementasi Eloquent, binding di `RepositoryServiceProvider`, Form Request, API Resource, Inertia + shadcn-vue sesuai `DESIGN.md`):
- **Daftar mesin**: nama, serial, region, status online/offline (`now() - last_seen_at < 3 × poll_delay_seconds`), versi firmware, jumlah user/jari, `last_seen_at`; filter region/aktif/offline. Empty state dengan aksi.
- **Tambah/ubah mesin** (pendaftaran manual, D2): serial (unik), nama, region, lokasi, zona waktu (daftar IANA, default `Asia/Jakarta`), aktif/nonaktif, `poll_delay_seconds`, `error_delay_seconds`. Mesin dinonaktifkan, tidak dihapus bila masih punya log.
- **Antrean perintah per mesin**: tombol REBOOT, INFO, CHECK, SET TIME, ADD USER (pilih pegawai; **validasi panjang/karakter `employee_number` terhadap batas PIN firmware**, Q3, dengan pesan yang jelas), ADD FP (pilih pegawai yang punya template; v2 hanya merujuk `biometric_template_id`). Aksi destruktif/berisiko lewat `ConfirmDialog`. Tampilkan status, percobaan, `return_code`, `result_message`. Batalkan perintah `pending` (`cancelled`). Tidak ada CLEAR LOG atau hapus user (D8). Pembuatan perintah dicatat ke `audit_logs`.
- **Log mentah bermasalah**: daftar `device_logs` dengan `outcome <> 'punched'` atau `is_suspect = true`, filter mesin/outcome/rentang waktu; aksi **"proses ulang"** per log atau per mesin (Q4) untuk outcome yang `isReprocessable()`.
- **Biometrik**: halaman pegawai menampilkan **metadata saja** (jenis, jumlah jari, mesin sumber, tanggal). Tombol hapus template per pegawai (dengan konfirmasi dan jejak audit). Tidak ada tampilan isi template.
- **Izin baru** (spatie/permission): `devices.view`, `devices.manage`, `devices.commands`, `device-logs.review`, `biometrics.manage`; seeder peran diperbarui. Cakupan data lewat `visibleTo($user)`.

### Langkah 5. Aksi "proses ulang" (aturan bersama dengan Rust)
Implementasikan sebagai Action (`ReprocessDeviceLog`) dengan **aturan identik** dengan alur ADMS (`schema-proposal.md` bagian 4, langkah 5-6). Urutan evaluasi dan hasil:

| Urutan | Kondisi | `outcome` |
|---|---|---|
| 1 | `device_time` < `2020-01-01` (A4, dapat diatur lewat `config/attendance.php`) | `out_of_range` (tidak dapat diproses ulang) |
| 2 | Tak ada pegawai dengan `employee_number = pin` (cocok persis, tanpa trim/padding) | `unknown_pin` |
| 3 | Pegawai ada, `is_active = false` | `employee_inactive` |
| 4 | Bulan `punched_at` di zona `attendance.timezone` berada di periode berstatus bukan `open` (gunakan `PeriodGuard` yang sama) | `period_closed` |
| 5 | Sudah ada punch `approved` dengan `(employee_id, punched_at)` sama | `duplicate` |
| 6 | selain itu | `punched`: buat `attendance_punches` (`source=device`, `status=approved`, kolom lain `NULL`) dan isi `device_logs.punch_id`, `employee_id` |

Seluruhnya dalam satu transaksi; pemrosesan ulang log yang sudah punya `punch_id` tidak boleh membuat punch kedua (dijaga unique parsial `punch_id`). `is_suspect` tidak mengubah hasil (D4b).

Tulis **tabel kasus uji bersama** (data, bukan kode) di `tests/contract/adms/outcome-cases.json` berisi kasus per baris di atas (+ tepi: pegawai aktif tapi periode `closing`, dua mesin waktu sama, PIN dengan nol di depan tidak sama dengan `employee_number` tanpa nol). Sesi ADMS menyalin berkas yang sama untuk test Rust-nya; ubahan aturan harus lewat PR pasangan.

### Langkah 6. Dump skema dan serah balik
1. Jalankan `php artisan migrate` di Postgres lokal, lalu `php artisan schema:dump --path=database/dumps/pgsql-schema.sql` (butuh `pg_dump`: `C:\Program Files\PostgreSQL\18\bin`).
2. Pastikan dump memuat empat tabel baru dan index parsial.
3. Jalankan seluruh test, `./vendor/bin/pint --test`, dan suite kontrak mobile (`bash scripts/contract-test.sh`). Periksa bahwa `eabsensi-mobile-api` menerima `source = 'device'` pada daftar punch (hanya baca `../eabsensi-mobile-api`; bila ada `enum`/`match` yang menolak, catat sebagai PR pasangan, jangan ubah repo itu dari sini).
4. Setelah ter-merge: laporkan **commit SHA atau tag** yang memuat dump. Nilai itulah yang diisi ke `SCHEMA_VERSION` di `eabsensi-adms`.

### Jangan lakukan
- Jangan menulis kode protokol iClock/ADMS atau endpoint `/iclock/*` di v2.
- Jangan mengubah/menghapus kolom atau tabel yang sudah ada; jangan mengubah `attendance_punches` (tanpa kolom baru).
- Jangan mendekripsi, menampilkan, mengekspor, atau me-log isi template biometrik; jangan meminta kunci enkripsi.
- Jangan menyalin `employee_fp`, `trans_suspects`, `device_temp`, atau `serial_list` lama; keputusan produk sudah menggantinya.
- Jangan mengubah repo `eabsensi-adms`, `eabsensi-mobile-api`, atau acuan lama.

---

## Ringkasan untuk pembaca manusia

| Hal | Isi |
|---|---|
| Tabel baru | `devices`, `biometric_templates`, `device_commands`, `device_logs` |
| Enum | `PunchSource::Device` + 5 enum baru |
| UI | daftar/form mesin, antrean perintah, log bermasalah + proses ulang, metadata biometrik |
| Izin baru | `devices.view/manage/commands`, `device-logs.review`, `biometrics.manage` |
| Serah balik | dump skema + commit SHA/tag untuk `SCHEMA_VERSION` |

## 6. Rincian persetujuan user (25 Oktober 2026)

- **A1-A6** (Q1): disetujui sebagaimana tertulis di `schema-proposal.md` bagian 1.
- **Q2** `region_id` nullable untuk pengelompokan mesin: disetujui.
- **Q3** validasi panjang/karakter PIN di UI v2 saat membuat `add_user`: disetujui (Langkah 4).
- **Q4** proses ulang dari UI v2: disetujui (Langkah 4-5).
- **Q5** retensi `device_logs`: ditunda ke Task 7 (operasional); tidak ada pekerjaan di v2 sekarang.
- **Q6** kebijakan hapus template dan persetujuan pegawai (UU PDP): **keputusan kebijakan di luar kode**, belum ada pemilik; v2 menyediakan penghapusan manual per pegawai saja. Perlu ditindaklanjuti oleh pemilik produk sebelum produksi.
- **Q7** pemegang kunci enkripsi, cadangan, dan rotasi: ditunda ke Task 7; tidak ada pekerjaan di v2.
- **Q8** foto wajah: disetujui dengan **tafsir**: jenis `face_photo` tetap ada di skema, tetapi ADMS **hanya menyimpannya bila diaktifkan** lewat konfigurasi (`BIOMETRIC_STORE_FACE_PHOTO`, default `false`). Default sistem hanya menyimpan template jari dan wajah. (Menyelaraskan A6 yang semula menyatakan foto wajah ikut disimpan; ubah bila tafsir ini salah.)
