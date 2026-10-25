# Analisis acuan lama `eabsensi-99` (ADMS / iClock)

Dibuat: 25 Oktober 2026 (Task 1). Hanya bacaan; `../eabsensi-99` dan `../eabsensi` tidak diubah.

## 0. Sumber, cakupan, dan batas dokumen ini

| Sumber | Dipakai untuk |
|---|---|
| `../eabsensi-99/routes/web.php` | Daftar rute `iclock/*` |
| `../eabsensi-99/app/Http/Controllers/IDeviceController.php` (1.579 baris) | Seluruh perilaku endpoint. Rujukan `IDC:<baris>` di bawah menunjuk file ini |
| `app/*.php` (model), `app/Services/SendNotif.php`, `SendSuspectNotif.php`, `ApiHttpBasic.php`, `app/Helper.php`, `app/Http/Middleware/DevicesCon.php`, `VerifyCsrfToken.php` | Tabel, notifikasi, fungsi waktu/acak, perlakuan HTTP |
| `../eabsensi/app/Http/Controllers/IDeviceController.php` (375 baris) | Pembanding: versi lama yang lebih sederhana |
| `../eabsensi-99/storage/logs/laravel-*.log` | Bukti nyata dari mesin produksi: baris request, `User-Agent`, panjang body, stamp |

**Asal contoh payload (penting).** Log yang tersisa (versi kode yang lebih lama dari yang ada sekarang, saat itu `Log::info` mencetak objek `Request`) hanya memuat baris request dan header, **bukan isi body**. Jadi:

- Yang **nyata (tangkapan log produksi)**: URL + query string, `User-Agent`, `Content-Length`, nilai stamp, dan baris balasan `C:<id>:CHECK`. Nomor seri pada contoh sudah diganti `SN-CONTOH-0001`.
- Yang **disusun dari kode** (ditandai "disusun dari kode"): isi body (`ATTLOG`, `OPLOG`, `FP`, dst.) dan balasan handshake. Bentuknya mengikuti apa yang dibaca/ditulis kode lama dan format protokol iClock/ADMS umum. Bentuk baris pada firmware nyata harus dicocokkan dengan **tangkapan asli atau mesin fisik** sebelum Task 4/5 dianggap benar (lihat bagian 9).

Tabel Oracle lama memakai awalan `tm_` (`DB_PREFIX`/`DB_TABLE_PREFIX=tm_`, terlihat dari sequence `tm_transaction_logs_id_seq`). Skema tidak ada di migration; kolom diambil dari `$fillable`, `where`, `update`, dan `select` di kode.

---

## 1. Lapisan HTTP umum

- **Rute** (`routes/web.php:15-25`), semuanya di grup `iclock` dengan middleware `devcon`:

| Metode | Path | Method controller |
|---|---|---|
| GET | `/iclock/cdata` | `iconnect` (handshake) |
| GET | `/iclock/getrequest` | `iget` (polling perintah + heartbeat) |
| POST | `/iclock/cdata` | `idata` (unggah data) |
| POST | `/iclock/fdata` | `iphoto` |
| POST | `/iclock/devicecmd` | `icommand` (hasil perintah) |
| POST | `/iclock/registry` | `iregister` |
| POST | `/iclock/push` | `ipush` |
| GET | `/iclock/ping` | `iping` |
| GET | `/iclock/rtdata` | `iset` (sinkron waktu) |
| POST | `/iclock/querydata` | `iquery` |

- **Middleware**: `DevicesCon` (global) memaksa `session.driver=array` untuk `iclock/*` (tanpa sesi/cookie). `VerifyCsrfToken` mengecualikan `iclock/*`. Tidak ada autentikasi apa pun.
- **Pengenal mesin**: parameter query `SN` (nomor seri). **Satu-satunya "autentikasi"** per endpoint adalah `User-Agent` mengandung `iClock` (`Str::contains`). UA produksi yang tertangkap: `iClock Proxy/1.09`. Mudah dipalsukan.
- **Header request mesin (nyata, dari log)**: `Accept: */*`, `Connection: close`, `Content-Type:` **kosong**, `Host: <ip-server>:80`. Konsekuensi untuk Rust: **jangan bergantung pada `Content-Type`**; body mentah dibaca apa adanya. Server harus melayani **port 80** (atau mesin dikonfigurasi dengan port lain), HTTP/1.1, koneksi ditutup tiap request.
- **Balasan**: hampir semua `Content-Type: text/plain`, status 200. Bila kondisi tidak terpenuhi (tak ada `SN`, UA salah, mesin tak dikenal, dst.) fungsi mengembalikan `null` → Laravel membalas **200 dengan body kosong** (bukan `text/plain`). Beberapa jalur melempar error PHP → **500** (lihat bagian 8).
- **Zona waktu**: `config/app.php` `Asia/Jakarta`. Semua `date('Y-m-d H:i:s')` memakai waktu lokal server; waktu log dari mesin disimpan sebagai **string waktu lokal apa adanya** (tanpa zona).
- **Terkait env**: `TIMEZONE` (jam, dipakai `jamput()`), `API_URI`, `API_USERNAME`, `API_PASSWORD`, `API_SUSPECT_USERNAME`, `API_SUSPECT_PASSWORD`, `HTTP_PROXY`, `CURL_CONNECT_TIMEOUT` (10), `CURL_TIMEOUT` (30). `APP_DEBUG=true` di `.env` produksi.

## 2. Waktu dan stamp

### 2.1 Pengodean waktu ZK
`jamput()` (`Helper.php`) mengodekan waktu server ke "detik ZK":

```
detik = (((tahun-2000)*12*31 + (bulan-1)*31 + (hari-1)) * 86400) + jam*3600 + menit*60 + detik
detik_dikirim = detik - (env TIMEZONE * 3600)
```

(bulan dianggap 31 hari; ini pengodean asli firmware ZK). Dipakai pada `SET OPTIONS DateTime=<n>` dan `rtdata`.

**Bukti dari log produksi** (5 Jun 2024): stamp yang dikirim mesin `Stamp=785168153` dan `OpStamp=785154440`. Hari ke-`785168153/86400 = 9087` = `(24*372 + 4*31 + 4)` = **5 Juni 2024** persis, sisa 51.353 detik ≈ 14:15:53. Jadi stamp dari mesin berada di pengodean ZK di atas, bukan epoch Unix.

### 2.2 Stamp buatan server (kemungkinan salah)
Server menghitung stamp sendiri dengan `strtotime('now') - 935082000` untuk `operation_flag` (`IDC:1041`), `photo_flag` (`IDC:1121`, `1135`). Epoch Unix dikurangi konstanta tetap **tidak sama dengan** pengodean ZK (bulan 31 hari). Untuk 5 Juni 2024 16:13 WIB hasilnya ≈ `782.496.780`, sedangkan pengodean ZK ≈ `785.175.192`: selisih ≈ **31 hari**. Artinya stamp yang dikembalikan server di handshake tertinggal sekitar sebulan dari yang dimaksud. Stamp ATTLOG tidak pernah disimpan (baris `IDC:1117` dikomentari), sehingga `Stamp=None` terus dikirim di handshake. Akibat yang terlihat: batch yang sama dikirim ulang oleh mesin pada setiap koneksi ulang (log 5 Jun 2024: `ATTLOG&Stamp=785168153`, `Content-Length: 1490`, muncul lagi pada 16:13 dan 16:19; log 9 Jul 2025: 58 waktu absen unik muncul masing-masing dua kali = 116 baris). **Penyebab pasti belum diverifikasi** dengan mesin; disimpan sebagai hipotesis.
- Konstanta lain: `time_second + 933613200` di `querydata transaction` (`IDC:1519`) memakai konstanta **berbeda** dari 935082000 dan juga tidak membalik pengodean ZK → hampir pasti salah.

**Implikasi desain** (diteruskan ke Task 2/4): server baru menyimpan stamp yang dikirim mesin (`Stamp`, `OpStamp`, `PhotoStamp`, `BIODATAStamp`) dan mengembalikannya apa adanya; tidak menghitung stamp sendiri.

---

## 3. Endpoint satu per satu

Notasi: `\t` = tab, `\n` = LF. `[iClock]` = UA harus mengandung `iClock`.

### 3.1 `GET /iclock/cdata`: handshake (`iconnect`, `IDC:31-248`)

**Request nyata (log produksi, SN diganti):**
```
GET /iclock/cdata?SN=SN-CONTOH-0001&options=all&pushver=2.1.44&language=69 HTTP/1.1
Accept: */*
Connection: close
Content-Type:
Host: 10.15.64.204:80
User-Agent: iClock Proxy/1.09
```
Parameter lain yang dibaca kode: `pushver`, `PushOptionsFlag`, `DeviceType` (`att` atau `0`).

**Urutan keputusan:**
1. `device_list` harus punya baris `serial_no=SN` **dan** `is_active='1'`. Jika tidak, **200 body kosong**. (Konsekuensi: mesin non-aktif atau tak dikenal tidak pernah masuk ke `device_temp`; lihat bagian 8.)
2. Wajib ada `SN` dan `options`, dan `[iClock]`. Jika tidak, body kosong.
3. **Daftar putih nomor seri**: `snkey` dihitung dari `strtoupper(md5('snini' . SN . 'adalahkatakunci'))`, ambil 4 karakter pada offset 0, 8, 16, 24 lalu digabung (16 karakter hex). `snkey` harus ada di `serial_list.sid` (di-cache 900 detik, tapi `Cache::forget` dipanggil lebih dulu sehingga cache tidak pernah efektif). Jika tidak ada → body kosong.
4. **Cabang firmware push 3.x** (`pushver` diawali `3.` **dan** `PushOptionsFlag=1`): balasan `OK`; jika `snkey` valid dan baris `device_list` ada, `server_ver=pushver`, `status=1`, `last_activity=now`, balasan menjadi `OK\nPushProtVer=<pushver>\nPushOptionsFlag=1`. (Cabang `device_temp` di dalamnya tidak bisa tercapai karena langkah 1.) *Disusun dari kode:*
```
OK\nPushProtVer=3.0.1\nPushOptionsFlag=1
```
5. **Cabang lain**, tergantung `device_list`:
   - **(a) `is_new = true`**: balasan opsi lengkap (di bawah). Efek samping: `status=1`, `last_activity=now`; buat (sekali, `firstOrCreate`) dua baris `command_list`: `INFO` (`data='INFO'`, `status=1`, `flag=1`) dan `SET ABCPIN` (`data='SET OPTION IsSupportABCPin=1'`, `flag=2`); isi cache `AREA<sn>` dan `DEVNAME<sn>`. Jika `DeviceType` (dari request) `att` atau `0`: balasan format "att" dan `device_list.device_type='att'`.
   - **(b) `is_new=false` dan `push_version IS NULL`** (firmware lama): opsi format lama.
   - **(c) `is_new=false` dan `push_version IS NOT NULL`**: format dengan `PushProtVer=2.3.0`; bila `device_list.device_type='att'` → format "att".
   - **(d) selain itu** (mis. `is_new IS NULL`): upsert `device_temp` (`serial_no`, `ip_address`, `connection_timestamp`), body kosong.

**Balasan persis, format lama (a tanpa att, b)** *(disusun dari kode; `<…>` dari `device_list`; stamp kosong → `None`)*:
```
GET OPTION FROM:SN-CONTOH-0001\n
Stamp=<transaction_flag|None>\n
OpStamp=<operation_flag|None>\n
PhotoStamp=<photo_flag|None>\n
ErrorDelay=<err_delay>\n
Delay=<delay>\n
TransTimes=00:00;14:05\n
TransInterval=1\n
TransFlag=111111111111\n
TimeZone=<timezone>\n
Realtime=1\n
Encrypt=0\n
```
**Format "att"** (`DeviceType=att|0`, atau `device_type='att'`):
```
GET OPTION FROM:SN-CONTOH-0001\n
ATTLOGStamp=<transaction_flag|None>\n
OPERLOGStamp=<operation_flag|None>\n
BIODATAStamp=<bio_flag|None>\n
ATTPHOTOStamp=<photo_flag|None>\n
ErrorDelay=<err_delay>\n
Delay=<delay>\n
TransTimes=00:00;14:05\n
TransInterval=1\n
TransFlag=111110101101\n
TimeZone=<timezone>\n
Realtime=1\n
Encrypt=0\n
ServerVer=adms2.0\n
PushOptions=RegDeviceType,FingerFunOn,FaceFunOn,FPVersion,FaceVersion,NetworkType\n
MultiBioDataSupport=0:1:1:0:0:0:0:0:1:1\n
MultiBioPhotoSupport=0:0:0:0:0:0:0:0:0:1\n
```
**Format (c) non-att** menambah setelah `Delay`: `PushProtVer=2.3.0\n`, `ServerVer=2.3.0\n`, `EncryptFlag=1000000000\n`, dan `TransFlag=TransData AttLog\tOpLog\tAttPhoto\tEnrollFP\tEnrollUser\tFPImag\tChgUser\tChgFP\tFACE\tUserPic\tFVEIN\tBioPhoto\n` (`IDC:183-197`).

Catatan: `TransTimes` memakai `;` di handshake tetapi `\t` di `push` (`00:00\t14:00`).

### 3.2 `GET /iclock/getrequest`: polling perintah + heartbeat (`iget`, `IDC:251-344`)

Request biasa: `GET /iclock/getrequest?SN=<sn>` (poll rutin; ±292 poll di satu hari log 2025). Variasi `&INFO=<fw>,<jumlah_user>,<jumlah_fp>,<jumlah_transaksi>,<ip>,...` (laporan info dari mesin).

1. Jika ada `SN` **dan** `INFO` (tanpa cek UA): `device_list.status=1`, `last_activity=now`.
2. Jika `SN`+`INFO`+`[iClock]`: `explode(',', INFO)` → `firmware_version=[0]`, `user_count=[1]`, `fp_count=[2]`, `transaction_count=[3]`, `ip_address=[4]` (tanpa cek panjang). Balasan `OK`.
3. Jika `SN`+`[iClock]` (tanpa `INFO`): **antrean perintah** dengan prioritas kelas `flag`; hanya kelas non-kosong pertama yang dikirim, `status=1` → dikirim lalu `status=2`:

| flag | Arti (dari komentar kode) | Batas per poll |
|---|---|---|
| 1 | perintah sistem | 1 |
| 2 | pengaturan | 10 |
| 3 | data user | 50 |
| 4 | data sidik jari | 5 |
| 5 | data wajah | 5 |
| 6 | data foto | 5 |

   Urutan `ORDER BY id`. Format baris: `C:<command_list.id>:<data>\n`. **Nyata (log):** `C:90470:CHECK\n`.
4. Jika kelas 1-6 kosong, dan `commands_user` punya baris `status=0` untuk SN itu (join `employee_list` pada `nrk=emp_id` untuk nama): 
```
ADD USER → C:<id>:DATA USER PIN=<nrk>\tName=<emp_name>\tPri=0\n
ADD FP   → C:<id>:DATA FP PIN=<nrk>\tFID=<fid>\tSize=<size>\tValid=1\tTMP=<bio_tmp>\n
REBOOT   → C:<id>:REBOOT\n
```
   Baris `commands_user` **tidak diberi status terkirim**; terkirim ulang tiap poll sampai `devicecmd` melapor (lihat bagian 4).
5. Tidak ada perintah → balasan `OK`. Setelah itu: bila balasan ≠ `OK` → `status=2` (sibuk), selain itu `status=1`.

Balasan selalu `text/plain`, 200.

### 3.3 `POST /iclock/cdata`: unggah data (`idata`, `IDC:347-1129`)

Semua cabang butuh `SN` dan `[iClock]`. Percabangan berdasarkan `table`, urutan `if` penting:

| # | Kondisi | Efek |
|---|---|---|
| 1 | `table=rtstate` | Balasan `OK`, tidak ada efek. |
| 2 | `table=options` | Body `k=v,k=v,...` → update kolom info `device_list`, balasan `OK`. |
| 3 | `table=rtlog` | Log realtime (lihat 3.3.2). Balasan `OK`. |
| 4 | `table=tabledata` | `tablename` ∈ `user`, `extuser`, `templatev10`, `biophoto`, `userpic`, `ATTPHOTO` (3.3.4). |
| 5 | `table` ∈ {`OPERLOG`,`USERINFO`,`FINGERTMP`} **atau** ada `OpStamp` | Log operasi + data biometrik (3.3.3). |
| 6 | `table=BIODATA` | Hanya menyimpan `bio_flag=Stamp`; body diabaikan. |
| 7 | `table=ATTLOG` **atau** (ada `Stamp` dan `table` bukan OPERLOG/USERINFO/FINGERTMP/ATTPHOTO) | **Log absensi** (3.3.1). |
| 8 | `table=ATTPHOTO` atau ada `PhotoStamp` | `photo_flag=now-935082000`; foto tidak disimpan. |

**Balasan akhir semua cabang** (kecuali #1, #2, #3 yang `return` langsung): jika `$count > 0` → `OK:<count>\n`, selain itu `OK\n`. Perhatikan: pada cabang `tabledata`, nilai `$rsp = "<tablename>=<count>"` **ditimpa** oleh balasan akhir ini (`IDC:1124-1127`), sehingga tidak pernah terkirim.

#### 3.3.1 `ATTLOG`: log absensi (`IDC:1046-1117`)

Request nyata (log produksi): `POST /iclock/cdata?SN=SN-CONTOH-0001&table=ATTLOG&Stamp=785168153`, `Content-Length: 1490`, `Content-Type` kosong. Body *(disusun dari kode: kolom 0=PIN, 1=waktu, 2=status/`function_key`, 3=verifikasi/`verify_mode`; kolom tambahan diabaikan)*:
```
12345\t2024-06-05 07:01:03\t0\t1\t0\t0\t0\n
12346\t2024-06-05 07:02:41\t0\t15\t0\t0\t0\n
```
Per baris tidak kosong:
1. `emp_id = str_pad(kolom0, 6, '0', LEFT)` (PIN diisi nol di kiri sampai 6 digit; NRK lama 6 digit).
2. Hanya jika `strtotime(kolom1) >= 2020-01-01 00:00:00`: cek `device_list.status` ∈ {0,1} (nilai `0` ikut ditolak oleh pengecekan `$status &&`, jadi efektifnya hanya `1`) **dan** `is_active=1`. Jika lolos: `TransactionLogs::firstOrCreate` dengan kunci `(emp_id, trans_datetime)` (**tanpa `serial_no`**) dan nilai `function_key`, `verify_mode`, `serial_no`, `submit_datetime=now`, `ip_address`, `flag=0`, `created_by='DEVICE'`, `created_at=now`, `id` dari sequence. Jika baris baru → `sendNotif` (3.8).
3. **Terlepas dari status/aktif**: jika `strtotime(kolom1) > now + 500 detik` → `TransSuspects::firstOrCreate` (kunci sama) dan bila baru → notifikasi curiga (3.8).
4. `$count++` untuk setiap baris tidak kosong, **berhasil disimpan atau tidak**. `try/catch(Throwable)` membungkus tiap baris dan hanya mencatat `Log::error('gagalMasuk')`.
5. `stamp` dihitung tapi **tidak disimpan** (komentar `IDC:1117`).
6. Balasan `OK:<jumlah baris>\n`.

#### 3.3.2 `rtlog`: log realtime (`IDC:392-459`)

Body baris-baris `k=v` dipisah tab, mis. *(disusun dari kode)*:
```
time=2024-06-05 07:01:03\tpin=12345\tinoutstatus=0\tverifytype=1\teventflag=0\tevent=0\tmaskflag=0\ttemperature=36.5\n
```
Kunci yang dibaca: `pin`, `time`, `inoutstatus`→`function_key`, `verifytype`→`verify_mode`, `eventflag`→`flag`, `event`, `maskflag`, `temperature`. Logika sama dengan ATTLOG tetapi: `emp_id = pin` **tanpa padding**, tidak ada cek `is_active` dan tidak ada batas 2020, **tanpa try/catch**. Setelah loop `status=1`, balasan `OK`.

#### 3.3.3 `OPERLOG` / `USERINFO` / `FINGERTMP` / `OpStamp` (`IDC:814-1042`)

Request nyata: `POST /iclock/cdata?SN=SN-CONTOH-0001&table=OPERLOG&OpStamp=785154440` (`Content-Length: 3744`) dan `...&OpStamp=785166214` (`Content-Length: 114`). Baris berawalan:

| Awalan | Format (disusun dari kode) | Efek |
|---|---|---|
| `OPLOG ` | `OPLOG <op_id>\t<admin>\t<waktu>\t<obj1>\t<obj2>\t<obj3>\t<obj4>` | Jika waktu ≥ 2022-01-01: `OperationLogs::firstOrCreate` kunci `(serial_no, log_datetime)`; `admin_id` = hanya digit |
| `USER ` | `USER PIN=..\tName=..\tPri=..\tPasswd=..\tCard=..\tGrp=..\tTZ=..` | **Dinonaktifkan** (kode dikomentari): tidak menyimpan pegawai |
| `FP ` | `FP PIN=..\tFID=..\tSize=..\tValid=..\tTMP=<base64>` | upsert `employee_fp` (`bio_type=1`) |
| `FACE ` | `FACE PIN=..\tFID=..\tSIZE=..\tVALID=..\tTMP=<base64>` | upsert `employee_fp` (`bio_type=2`) |
| `BIOPHOTO ` | `BIOPHOTO PIN=..\tFileName=..\tType=..\tSize=..\tContent=<base64>` | tulis file `biophoto/<FileName>` + upsert `employee_fp` (`bio_type=3`) |
| `USERPIC` | | Dinonaktifkan |

Per baris `$count++` (termasuk baris kosong sisa `explode`). Akhirnya `operation_flag = now - 935082000`, `status=1`. Pengaksesan `$data[2]..$data[6]` pada `OPLOG` **tidak** dibungkus try/catch.

#### 3.3.4 `table=tabledata` (`IDC:460-813`)
`tablename` dan `count` dari query. `user`, `extuser`, `userpic`, `ATTPHOTO`: hanya `status=1`/`last_activity`. `templatev10`: upsert `employee_fp` (`bio_type=1`) dari baris `templatev10 pin=..\tfingerid=..\tsize=..\tvalid=..\ttemplate=<base64>`. `biophoto`: simpan file + upsert `employee_fp` (`bio_type=3`) dari `biophoto pin=..\tfilename=..\ttype=..\tsize=..\tcontent=<base64>`. Balasan efektif `OK\n` (lihat catatan penimpaan di atas).

#### 3.3.5 `table=options` (`IDC:358-391`)
Body satu baris `k=v` dipisah **koma** *(disusun dari kode; kunci yang dibaca)*:
`PushVersion, UserCount, FingerFunOn, FPCount, ~MaxFingerCount, PhotoFunOn, FaceFunOn, FaceVersion, FaceCount, ~MaxFaceCount, FvFunOn, FvVersion, FvCount, ~MaxFvCount, PvFunOn, PvVersion, PvCount, ~MaxPvCount, TransactionCount, ~MaxAttLogCount, ~DeviceName, Language, ~Platform, ~OEMVendor, IPAddress` → kolom `device_list` bernama sama (lihat bagian 5). Balasan `OK`. Nilai yang mengandung `&` atau `=` akan merusak `parse_str`.

### 3.4 `POST /iclock/fdata` (`iphoto`, `IDC:1131-1140`)
Dengan `SN`+`[iClock]`: `photo_flag = now-935082000`, `status=1`. Body (foto absensi) **diabaikan**. Balasan selalu `OK\n`.

### 3.5 `POST /iclock/devicecmd`: hasil perintah (`icommand`, `IDC:1142-1275`)

Request nyata: `POST /iclock/devicecmd?SN=SN-CONTOH-0001`, `Content-Length: 28`. 28 byte = tepat `ID=90470&Return=0&CMD=CHECK\n` (27 karakter + LF); cocok dengan perintah `C:90470:CHECK` dari getrequest. Format hasil standar: satu baris per perintah `ID=<id>&Return=<kode>&CMD=<nama>` (+ bidang tambahan).

- **Body mengandung `CMD=INFO`**: tiap baris `k=v` (digabung `&`) → update kolom info `device_list` (`PushVersion, UserCount, FingerFunOn, ~ZKFPVersion, FPCount, ~MaxFingerCount, ..., TransactionCount, ~MaxAttLogCount, ~DeviceName, FlashSize→memory_size, FreeFlashSize→memory_free, Language, VOLUME→volume, ~Platform, ~OEMVendor`). `command_list[ID]` → `status=0`, `return_time`, `return_value=Return`. Bila `device_list.is_new=1` → `is_new=0` dan buat perintah `CHECK` (`flag=1`).
- **Lainnya**, per baris (`parse_str`):
  - `Return == 0`: `command_list[ID]` `status=0` + waktu/nilai balik; jika `CMD` `DATA` atau `REBOOT` juga `commands_user[ID]` `status=1`.
  - `Return != 0`: `CMD='DATA QUERY'` → `command_list` `status=0`; `CMD` `DATA`/`REBOOT` → `commands_user` `status=1` (**gagal pun ditandai 1**); selain itu `command_list` `status=9`, `command = command || ', (id-Failed)'`, `return_value` = hanya digit dan `-`.
- Balasan selalu `OK\n`; kesalahan hanya dicatat (`Log::error('errorid')`).

### 3.6 `POST /iclock/registry` (`iregister`, `IDC:1277-1332`)
Dengan `SN`+`[iClock]`: jika `device_list` tidak punya SN → balasan `UNKNOWN DEVICE` (200). Jika ada: body `k=v,k=v` (kunci seperti `options` plus `DeviceType`, `~ZKFPVersion`, `FlashSize`, `FreeFlashSize`, `VOLUME`) → update info; `device_type` = `acc` bila `DeviceType=1`, selain itu nilai mentah; `registry_code` = kode lama atau acak 10 karakter alfanumerik (`kocok()` memakai `rand()`, bukan CSPRNG). Balasan `RegistryCode=<kode>` (tanpa newline).

### 3.7 `POST /iclock/push`, `GET /iclock/rtdata`, `GET /iclock/ping`, `POST /iclock/querydata`

**`push`** (`IDC:1334-1403`): dengan `SN`+`[iClock]` baris `device_list` harus ada (kalau tidak → error PHP/500). Balasan *(disusun dari kode)*:
```
ServerVersion=<server_ver>\n
ServerName=ADMS\n
PushVersion=<server_ver>\n
ErrorDelay=<err_delay>\n
RequestDelay=<delay>\n
TransTimes=00:00\t14:00\n
TransInterval=1\n
TransTables=User\tTransaction\n
SessionID=<10 karakter acak>\n
TimeoutSec=10
```
(tanpa newline akhir). Efek: `firstOrCreate` perintah `SET TIME` (`SET OPTIONS MachineTZ=<+HHMM>`, `flag=2`) dan `SET DATETIME` (`SET OPTIONS DateTime=<jamput()>`, `flag=2`); karena `firstOrCreate` dengan kunci `(serial_no, command)`, keduanya hanya dibuat **sekali seumur hidup mesin**. `status=1`, cache `AREA/DEVNAME/DEVID`.
Format zona `±HHMM` dari `device_list.timezone` (jam, boleh pecahan): `+0700`, `+0530`, `-0330`.

**`rtdata?type=time`** (`IDC:1405-1432`): balasan `DateTime=<jamput()>,ServerTZ=<±HHMM>` (tanpa newline). SN tak dikenal → error. Tipe lain → body kosong.

**`ping`**: `OK` (tanpa cek apa pun, tanpa heartbeat).

**`querydata`** (`IDC:1439-1564`), butuh `SN`+`[iClock]`:
- `type=tabledata`: `tablename=user|extuser|userpic` → `status=1`; `templatev10`, `biophoto` → sama seperti cdata; `transaction` → baris `pin, time_second, inoutstate, verified, eventflag, event, maskflag, temperature`: `dtime = date('Y-m-d H:i:s', time_second + 933613200)` (konstanta salah, 2.2), `TransactionLogs::firstOrCreate` kunci `(emp_id=pin tanpa padding, trans_datetime)`, `sendNotif($pin, $dtuser['time'])` memakai kunci `time` yang tidak ada di baris ini (jadi Carbon::parse(null) = sekarang, tanggal/jam notifikasi salah). Balasan efektif: `<tablename>=<count>`.
- `type=options`: balasan `OK`.
- `type=count`: body `user=N` / `templatev10=N` / `biophoto=N` → `user_count`, `fp_count`, `face_count`; balasan `OK`.
- lainnya: body kosong.

### 3.8 Notifikasi (`sendNotif`, `IDC:1566-1578`, `SendNotif`, `SendSuspectNotif`, `ApiHttpBasic`)
Dipanggil **sinkron di tengah request mesin**. Menembak `POST` `application/x-www-form-urlencoded` ke `API_URI` (HTTP Basic, lewat proxy `HTTP_PROXY`, default `10.15.3.21:80`) dengan bidang:
- Absen biasa: `kirimNotif=1`, `tujuan=<emp_id>`, `isi="Anda absen fingerprint pada {d-M-Y} pukul {H:i:s}"` (kredensial `API_USERNAME/PASSWORD`).
- Mencurigakan: `kirimNotif=1`, `tujuan=181164` (**hard-coded `$sendTo`**), `isi="Absen mencurigakan oleh {pin} di {tgl} pada pukul {jam}"` (kredensial `API_SUSPECT_*`).
Batas waktu cURL 10 detik koneksi / 30 detik total; kegagalan cURL hanya membuat array `status=error`, tidak dilempar. Nilai `batasWaktu = 15` (menit) dideklarasikan tetapi tidak dipakai (cek lama dikomentari).

---

## 4. Antrean perintah

### 4.1 `command_list` (perintah umum)
- Kolom: `id`, `serial_no`, `command` (nama unik per mesin; kunci `firstOrCreate`), `data` (teks yang dikirim ke mesin), `status`, `flag`, `submit_time`, `return_time`, `return_value`.
- Siklus `status`: `1` menunggu → (dikirim di getrequest) `2` → (hasil di devicecmd) `0` selesai / `9` gagal.
- `flag` = kelas prioritas (tabel 3.2).
- Perintah yang dibuat otomatis oleh server: `INFO`, `SET ABCPIN`, `CHECK`, `SET TIME`, `SET DATETIME`. Perintah operator (isi `DATA USER`, `DATA FP`, dst.) dibuat dari aplikasi web lama (tidak ada di repo ini).
- Karena `firstOrCreate` pada `(serial_no, command)`, perintah dengan nama sama **tidak bisa diulang** setelah ada barisnya (mis. `CHECK` kedua tidak dibuat bila baris `CHECK` lama masih ada). Di log terlihat `C:90470:CHECK` lalu `C:90471:CHECK` tiap ±6 menit, jadi ada proses luar (aplikasi web/cron) yang menghapus atau menambah ulang perintah tersebut; mekanismenya tidak ada di repo ini.

### 4.2 `commands_user` (perintah per pegawai)
- Kolom: `serial_no`, `command` (`ADD USER` | `ADD FP` | `REBOOT`), `nrk` (PIN pegawai), `status` (0 menunggu, 1 selesai), `flag`, `submit_time`, `return_time`, `return_value`; juga dibaca `fid`, `size`, `bio_tmp` (tidak ada di `$fillable`). Relasi `hasMany(EmployeeFP, emp_id ← nrk)` (`$with=['fp']`).
- Dikirim hanya bila `command_list` tidak punya perintah menunggu; tidak diberi status "terkirim", dan tidak dibatasi jumlahnya per poll.
- **Tabrakan ID**: `command_list.id` dan `commands_user.id` dari dua sequence berbeda, tetapi balasan `devicecmd` hanya membawa `ID` dan kode memperbarui **kedua** tabel dengan `ID` itu (lihat 3.5). Kolom `id` harus unik lintas dua tabel atau kedua tabel digabung.

---

## 5. Tabel dan kolom yang dipakai

Awalan Oracle `tm_` di semua tabel. "Aktif" = dipakai kode yang berjalan; "mati" = hanya di kode yang dikomentari.

### `device_list` (`DeviceList`): registri mesin
| Kolom | Arti / penggunaan |
|---|---|
| `id` | PK (cache `DEVID<sn>`) |
| `serial_no` | Nomor seri (kunci pencarian) |
| `is_active` | `'1'` agar mesin boleh handshake dan ATTLOG |
| `is_new` | Mesin baru terhubung pertama kali; `icommand INFO` menurunkannya ke 0 |
| `status` | 0/1 = idle, 2 = sedang menerima perintah; dipakai sebagai filter penerimaan log |
| `last_activity` | Waktu kontak terakhir (heartbeat) |
| `server_ver`, `push_version` | Versi push firmware |
| `transaction_flag`, `operation_flag`, `photo_flag`, `bio_flag` | Stamp ATTLOG / OPERLOG / ATTPHOTO / BIODATA |
| `err_delay`, `delay` | `ErrorDelay`, `Delay` di handshake (detik) |
| `timezone` | Jam (boleh pecahan) |
| `device_type` | `att` / `acc` / lain |
| `area_id`, `device_name` | Area dan nama (cache; area dipakai kode mati) |
| `firmware_version`, `ip_address`, `device_model`, `platform`, `oem_vendor`, `language`, `volume`, `memory_size`, `memory_free` | Info perangkat |
| `user_count`, `fp_count`, `face_count`, `vein_count`, `palm_count`, `transaction_count` | Jumlah data di mesin |
| `fp_fun`, `photo_fun`, `face_fun`, `vein_fun`, `palm_fun` | Kemampuan biometrik (0/1) |
| `fp_version`, `face_version`, `vein_version`, `palm_version` | Versi algoritma |
| `fp_maximum`, `face_maximum`, `vein_maximum`, `palm_maximum`, `transaction_maximum` | Kapasitas |
| `registry_code` | Kode registrasi (3.6) |
| `is_sync`, `is_sync_master` | Hanya di kode mati (sinkron user antar mesin) |

### `device_temp` (`DeviceTemp`): mesin menunggu persetujuan
`serial_no`, `ip_address`, `connection_timestamp`, `sn_key`. Dalam praktik tidak terisi lagi (bagian 8).

### `serial_list` (`SerialList`): daftar putih
`sid` = 16 karakter hex hasil hash nomor seri (3.1 langkah 3).

### `command_list`, `commands_user`: lihat bagian 4.

### `transaction_logs` (`TransactionLogs`): log mentah absensi
`id` (sequence `tm_transaction_logs_id_seq`), `emp_id` (PIN/NRK; ATTLOG dipad 6 digit), `trans_datetime` (string waktu lokal mesin), `function_key` (status masuk/keluar mesin), `verify_mode`, `serial_no`, `submit_datetime` (waktu server menerima), `ip_address` (IP mesin), `flag`, `event`, `maskflag`, `temperature`, `created_by` (`DEVICE`), `created_at`. Kunci dedup efektif: `(emp_id, trans_datetime)`.

### `trans_suspects` (`TransSuspects`)
Kolom sama dengan `transaction_logs` (sequence `tm_transaction_suspects_id_seq`). Berisi log dengan waktu > sekarang + 500 detik.

### `operation_logs` (`OperationLogs`)
`id` (sequence `tm_operation_logs_id_seq`), `serial_no`, `log_datetime`, `operation_id`, `admin_id`, `object_1..4`, `created_by`, `created_at`. Dedup `(serial_no, log_datetime)`.

### `employee_fp` (`EmployeeFP`): **data biometrik**
`emp_id`, `fid` (nomor jari; 0 untuk wajah/foto), `size`, `valid`, `bio_tmp` (template base64, atau nama file untuk foto), `bio_type` (1 sidik jari, 2 wajah, 3 foto), `created_by`, `created_at`, `serial_number`. Dedup `(emp_id, fid, bio_type)`; setiap mesin yang mengirim menimpa baris yang sama.

### `employee_list` (`EmployeeList`): hanya kode mati / join
`id`, `emp_id` (NRK = PIN), `emp_badge`, `emp_title`, `emp_name`, `emp_type`, `emp_pass`, `dept_id`, `area_id`, `cardno`, `privilege`, `group`, `timezone`, `created_by`, `created_at`; kode mati juga `verify`, `vicecard`, `photo_size`, `photo_img`, `updated_by/at`. Satu-satunya pemakaian aktif: join nama untuk perintah `ADD USER`.

### `employee_device` (`emp_id`, `serial_no`, `valid`), `employee_area` (`emp_id`, `area_id`, `status`)
Hanya kode mati (sinkron user antar mesin lewat `getareanodes(area_id)`, fungsi PL/SQL). Pegawai ↔ mesin/area **tidak** dipakai di jalur aktif.

### Lain
`PhotoLogs` (dirujuk kode mati, modelnya tidak ada); file di disk `biophoto/<nama>`; cache `serial_list`, `AREA<sn>`, `DEVNAME<sn>`, `DEVID<sn>` (hanya `AREA` yang dulu dibaca, oleh kode mati).

### Hubungan ke pegawai
`emp_id`/`pin`/`nrk` mesin = NRK pegawai (6 digit, nol di kiri dipad pada ATTLOG). Di v2 padanannya `employees.employee_number` (varchar 32). Tidak ada FK; log dengan PIN tak dikenal tetap tersimpan di `transaction_logs` (tidak ada validasi pegawai).

---

## 6. Fitur turunan

| Fitur | Perilaku lama | Status di kode |
|---|---|---|
| Pemetaan pegawai↔mesin/area (`EmployeeDevice`, `EmployeeArea`) | `firstOrCreate` saat `USER`/`tabledata user` masuk; area diturunkan ke anak area lewat `getareanodes` | **Mati** (dikomentari) |
| Sinkron user/sidik jari/wajah/foto antar mesin sebuah area | Membuat `command_list` flag 3/4/5/6 untuk mesin lain `is_sync` | **Mati** (dikomentari); sisa: kelas flag 3-6 di getrequest |
| Simpan template jari/wajah (`EmployeeFP`) | Upsert dari `FP`, `FACE`, `BIOPHOTO`, `templatev10`, `biophoto` | **Aktif** |
| Dorong user/jari ke mesin dari web (`commands_user`) | `ADD USER`/`ADD FP`/`REBOOT` | **Aktif** |
| Transaksi mencurigakan (`TransSuspects`) | Waktu log > sekarang + 500 detik → baris suspect + notifikasi ke penerima tetap | **Aktif** |
| Notifikasi absen ke pegawai | HTTP sinkron ke API notifikasi tiap log baru | **Aktif** |
| Auto-sinkron waktu | `SET TIME`/`SET DATETIME` sekali + `rtdata` | **Aktif** |
| Registrasi mesin baru | `device_temp` lalu operator pindahkan ke `device_list` | **Praktis tidak berfungsi** (bagian 8) |
| Foto absensi (`ATTPHOTO`, `fdata`) | Tidak disimpan | Mati |

---

## 7. Perilaku firmware yang ditangani kode lama

- UA `iClock Proxy/1.09`; `Content-Type` kosong; body mentah dibaca lewat `getContent()`.
- Dua keluarga firmware: *legacy push* (`Stamp/OpStamp/PhotoStamp`, `table=ATTLOG|OPERLOG`) dan *adms/att* (`ATTLOGStamp/OPERLOGStamp/BIODATAStamp/ATTPHOTOStamp`, `table=BIODATA`, `DeviceType=att|0`); firmware push 3.x memakai `pushver=3.*` + `PushOptionsFlag=1`.
- Stamp kosong dikirim `None`.
- Baris body dipisah LF; sisa `\r` pada baris terakhir tidak dibersihkan.
- Opsi (`options`, `registry`) dipisah koma, data (`ATTLOG`, `OPLOG`, `FP`) dipisah tab, base64 menerima spasi di tempat `+` (`str_replace(" ", "+")`).
- Setelah balasan perintah, mesin lazim membuka koneksi `cdata` baru (log: `CHECK` → handshake → `OPERLOG` → `ATTLOG` dalam 3 detik).
- Pada tiap perintah `CHECK`, mesin mengirim ulang batch yang sama (lihat 2.2).
- Balasan `OK:<n>` mengakui n baris (mesin menganggapnya diterima).

---

## 8. Bug, celah keamanan, dan keanehan (dengan rujukan baris)

**Kehilangan data / korektness**
1. **Log hilang diam-diam saat `status=2`.** ATTLOG (`IDC:1063`) dan rtlog (`IDC:403`) hanya menyimpan jika `status` 0/1, padahal `getrequest` menetapkan `status=2` tiap kali mengirim perintah. Log yang tiba saat itu dibuang, tetapi balasan tetap `OK:n` sehingga mesin menganggapnya sukses. (Tertutupi oleh pengiriman ulang batch yang sama, bukan dirancang.)
2. **Stamp ATTLOG tidak pernah disimpan** (`IDC:1117`) dan stamp buatan server salah ±31 hari (2.2) → mesin mengirim ulang batch yang sama berulang kali; beban dan duplikat bergantung pada `firstOrCreate`.
3. **Kunci dedup tanpa `serial_no`** `(emp_id, trans_datetime)`: absen dengan PIN dan detik yang sama dari dua mesin hanya menyimpan satu.
4. **`emp_id` tidak konsisten**: ATTLOG dipad 6 digit, `rtlog` dan `querydata transaction` tidak.
5. **`count` salah**: `$count++` untuk setiap baris, termasuk yang gagal/dibuang dan baris kosong (OPERLOG).
6. **Balasan `tabledata` ditimpa** oleh `OK`/`OK:n` akhir (`IDC:1124`), padahal kode membuatnya.
7. **Tabrakan ID** `command_list` vs `commands_user` (bagian 4.2); kegagalan perintah `DATA`/`REBOOT` ditandai `status=1` (sukses).
8. `commands_user` dikirim ulang tiap poll sampai dibalas; tanpa batas jumlah.
9. `SET TIME`/`SET DATETIME` hanya dibuat sekali (kunci `firstOrCreate`); jam mesin tidak disinkron ulang lewat jalur ini.
10. `time_second + 933613200` (querydata) tidak membalik pengodean ZK; `sendNotif` memakai kunci `time` yang tidak ada.
11. Waktu log disimpan sebagai string lokal tanpa zona; asumsi mesin satu zona dengan server.
12. `DeviceTemp` hampir mati: cabang `device_temp` hanya tercapai bila baris `device_list` sudah ada **dan** `is_active=1` (mustahil untuk mesin baru); mesin tak dikenal tidak terdaftar sama sekali. Commit terakhir ("Setting Non Aktif Mesin") menambah `is_active` tanpa menyesuaikan jalur ini.
13. Error PHP yang menghasilkan **500** ke mesin (membuat mesin menunda/mengulang): `INFO` kurang dari 5 field (`iget`), `OPLOG` kurang kolom, `push`/`rtdata` dengan SN tak dikenal, field `pin`/`time` hilang di `rtlog`.
14. `try/catch(QueryException)` kosong menelan error DB pada `employee_fp`.

**Keamanan**
15. **Tanpa autentikasi nyata**: UA yang bisa dipalsukan + SN yang bisa ditebak; endpoint selain handshake tidak cek daftar putih/`is_active` (kecuali ATTLOG), sehingga siapa pun yang tahu SN bisa menyuntik log (`rtlog`, `querydata transaction`) dan mengubah info mesin.
16. **Daftar putih memakai rahasia hard-coded** (`'snini'…'adalahkatakunci'`) di kode, hash MD5 dipotong. Kebocoran kode = lolos.
17. **Penerima notifikasi curiga hard-coded** (`181164`).
18. **Template biometrik** disimpan dan dikirim ulang; **file `biophoto/<FileName>`** ditulis dengan nama dari mesin tanpa validasi (Flysystem menolak `..`, tetapi tetap tanpa daftar putih ekstensi/ukuran).
19. **SQL ber-interpolasi** pada `getareanodes('".$arid->area_id."')` (kode mati; jangan dibawa).
20. `kocok()`/`petjah()` memakai `rand()` (bukan CSPRNG) untuk `RegistryCode`/`SessionID`.
21. `APP_DEBUG=true` di `.env` produksi; versi kode yang menulis log 2024 mencetak seluruh objek `Request` (header, IP) ke log harian, sedangkan kode sekarang mencatat penanda (`iget`, `idata`, waktu tiap baris, baris yang gagal beserta PIN).
22. Notifikasi sinkron di jalur request: API lambat/mati menahan balasan ke mesin hingga 30-40 detik per log baru.

**Beban**
23. Polling `getrequest` melakukan hingga 7 query `exists` + update tiap poll; tidak ada indeks/limit yang terdokumentasi.

---

## 9. Daftar fitur: pertahankan / buang / perlu keputusan

| # | Fitur | Label | Catatan untuk v2 |
|---|---|---|---|
| 1 | Handshake `cdata` GET dengan opsi per mesin (`Stamp`, `OpStamp`, `PhotoStamp`, `ErrorDelay`, `Delay`, `TransTimes`, `TransInterval`, `TransFlag`, `TimeZone`, `Realtime`, `Encrypt`, dan varian att) | **pertahankan** | Rakit dari tabel `devices`; byte-per-byte sama |
| 2 | Heartbeat + `last_seen_at` + status mesin offline | **pertahankan** | Bedakan "sibuk" dari "idle" tanpa menolak log |
| 3 | `getrequest` antrean perintah dengan kelas prioritas + `C:<id>:<cmd>` | **pertahankan** | Satu tabel perintah, ID unik; hindari tabrakan ID |
| 4 | `devicecmd` catat hasil (`status` selesai/gagal) | **pertahankan** | Pisahkan jelas gagal vs sukses |
| 5 | `ATTLOG` → log mentah idempoten → `attendance_punches` | **pertahankan** | Kunci unik `(serial, pin, waktu)` sesuai CLAUDE.md |
| 6 | Simpan stamp yang dikirim mesin dan kembalikan apa adanya | **pertahankan (perbaiki)** | Buang perhitungan `now - 935082000` |
| 7 | Sinkron waktu (`SET TIME`, `SET DATETIME`, `rtdata`) | **pertahankan (perbaiki)** | Kirim ulang berkala bila selisih jam besar |
| 8 | `options`/`registry`/`INFO`: info perangkat (versi, kapasitas, jumlah) | **pertahankan (subset)** | Cukup kolom yang dipakai UI web |
| 9 | `ping`, `push`, `registry` (RegistryCode), `rtstate` | **pertahankan** | Balasan sederhana dan kompatibilitas firmware |
| 10 | Daftar putih mesin (hanya mesin terdaftar boleh kirim) | **pertahankan konsep, buang mekanisme hash** | Ganti dengan tabel `devices` + `is_active`; tolak di semua endpoint |
| 11 | Hash `snini…adalahkatakunci` + `serial_list` | **buang** | Rahasia hard-coded |
| 12 | Tracking `status=2` sebagai penolak log | **buang** | Jangan menolak log; terima selalu |
| 13 | Cache `AREA/DEVNAME/DEVID/serial_list` | **buang** | Tidak dipakai jalur aktif |
| 14 | Sinkron user/jari/wajah/foto antar mesin (kode mati) dan `getareanodes` | **buang** | Kode mati; kembali hanya bila diputuskan |
| 15 | Kunci dedup tanpa `serial_no` | **buang** | Pakai `(serial, pin, waktu)` |
| 16 | Pendaftaran otomatis mesin baru (`device_temp`) | **perlu keputusan** | Praktis mati di kode lama; v2: daftar manual vs auto-pending |
| 17 | Simpan template jari/wajah/foto (`employee_fp`, `biophoto`, `FP`, `FACE`, `BIOPHOTO`, `templatev10`) dan dorong ke mesin (`ADD FP`) | **perlu keputusan** | Data biometrik sensitif; default usulan: tidak disimpan |
| 18 | `ADD USER` / `REBOOT` dari web (`commands_user`) | **perlu keputusan** | `REBOOT` berguna; `ADD USER` bergantung keputusan sinkron pegawai |
| 19 | `TransSuspects` (waktu > sekarang + 500 dtk) dan notifikasi curiga | **perlu keputusan** | Alternatif: tandai punch `pending`/`flagged` di v2, tanpa tabel terpisah |
| 20 | Notifikasi "Anda absen…" ke pegawai | **perlu keputusan** | Sinkron HTTP tidak dibawa; bila dipertahankan, lewat antrean asinkron/event v2 |
| 21 | `OPERLOG` / `OPLOG` (audit operasi di mesin) | **perlu keputusan** | Simpan atau abaikan |
| 22 | `rtlog` (log realtime akses/suhu/masker) | **perlu keputusan** | Hanya bila ada mesin yang memakainya; kolom `event/maskflag/temperature` |
| 23 | `querydata` (`transaction`, `count`, `tabledata`) | **perlu keputusan** | Tarik ulang data lama dari mesin; butuh dekode waktu ZK yang benar |
| 24 | `fdata`/`ATTPHOTO` (foto saat absen) | **perlu keputusan** | Hari ini dibuang; pilihan simpan atau tetap abaikan |
| 25 | PIN → pegawai: padding 6 digit, PIN tak dikenal | **perlu keputusan** | Pemetaan ke `employees.employee_number`, simpan log tak dikenal untuk ditinjau |
| 26 | Penanganan waktu: zona mesin, log dengan tanggal di masa depan/terlalu lama, bulan tertutup | **perlu keputusan** | Batas 2020/2022 di kode lama; v2 punya periode terkunci |
| 27 | Punch dari mesin langsung `approved`? | **perlu keputusan** | Skema `attendance_punches.status` default `approved` |

---

## 10. Hal yang belum terverifikasi (untuk Task 2/4/5/6)

1. **Isi body asli** `ATTLOG`, `OPLOG`, `rtlog`, `INFO`, `options`, dan balasan handshake pada firmware yang dipakai: perlu tangkapan nyata (mis. `tcpdump`/Wireshark ke server lama sekali, atau log body sementara di lingkungan uji) atau mesin fisik. Bentuk di dokumen ini disusun dari kode.
2. **Mekanisme pengiriman ulang batch** (hipotesis 2.2): uji dengan simulator dan mesin apakah mengembalikan `Stamp` apa adanya menghentikan pengiriman ulang.
3. **Keluarga firmware** yang benar-benar ada di lapangan (satu-satunya contoh log: `pushver=2.1.44`, `language=69`, `options=all`, UA `iClock Proxy/1.09`). Cabang push 3.x dan "att" belum punya bukti nyata.
4. **Penyebab perintah `CHECK` dibuat ulang tiap ±6 menit** (di luar repo ini; kemungkinan aplikasi web/cron).
5. **Apakah mesin memakai `Realtime=1`/`rtlog` benar-benar** pada armada saat ini (log 2024-2026 hanya menunjukkan `ATTLOG` dan `OPERLOG`).
6. **Skema Oracle sebenarnya** (tipe kolom, indeks, FK, nilai `status`/`flag`) tidak ada di repo; arti kolom di atas disimpulkan dari kode.
