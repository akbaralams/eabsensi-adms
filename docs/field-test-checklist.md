# Daftar periksa uji lapangan dengan mesin fisik

Task 7 meminta uji dengan minimal satu mesin fisik "bila tersedia". Saat pengembangan **tidak ada mesin fisik**, jadi semua perilaku firmware di bawah masih berupa **hipotesis** yang disusun dari kode lama, log produksi lama, dan simulator ([legacy-analysis.md](legacy-analysis.md) bagian 10; [implementation-notes.md](implementation-notes.md)). Dokumen ini memandu pengujian pertama dengan mesin sungguhan: apa yang diamati, apa hasil yang diharapkan, dan apa yang diubah bila hasilnya lain.

## Persiapan
1. Staging (atau produksi dengan satu mesin) sudah lolos pemeriksaan [operations.md](operations.md) bagian 10 langkah 1-5.
2. Daftarkan mesin di UI v2 (serial, region, zona waktu), pasang pegawai uji dengan `employee_number` = PIN yang akan dipakai.
3. **Rekam lalu lintas** antara mesin dan server. Lalu lintas berupa HTTP polos sehingga isi body terbaca; tidak perlu mengubah kode server (dan server memang tidak mencatat body):
   - Linux: `sudo tcpdump -i <antarmuka> -s 0 -w adms.pcap 'tcp port 8090 and host <ip mesin>'`
   - Windows: Wireshark dengan filter tangkap `tcp port 8090 and host <ip mesin>`, lalu *Follow → TCP Stream* untuk melihat teks request/respons.
   - **Jangan membagikan berkas capture mentah di luar tim**: berisi PIN pegawai dan, bila mesin mengirim sidik jari, template biometrik. Ambil hanya kutipan yang dibutuhkan dan samarkan PIN sebelum menempelkannya ke dokumen mana pun.
4. Siapkan akses ke log server (`journalctl -u eabsensi-adms -f` atau berkas NSSM) dan `psql`.

## Pengamatan (urutkan; catat hasil tiap baris)

| # | Pertanyaan | Cara mengamati | Hasil yang diharapkan | Bila lain |
|---|---|---|---|---|
| 1 | Mesin terhubung dan **bentuk handshake** diterima | capture `GET /iclock/cdata?SN=...&options=all&pushver=...`; balasan server; berhasil tidaknya mesin lanjut ke `getrequest` | mesin mengikuti `Delay` dan memanggil `getrequest` berkala | catat `pushver`, `DeviceType`, `PushOptionsFlag` dan balasan; cocokkan dengan bentuk di `protocol::handshake` (dasar, att, 2.3.0, push 3.x) |
| 2 | **Isi body ATTLOG** | capture `POST /iclock/cdata?table=ATTLOG` | `PIN\twaktu\tstatus\tverifikasi\t...` per baris | bila kolom/format berbeda, ubah `protocol::attlog::parse_attlog`; log server akan menunjukkan `rusak > 0` |
| 3 | **Stamp menghentikan pengiriman ulang?** | lakukan beberapa absensi, biarkan mesin 3-4 kali terhubung; hitung berapa kali batch yang sama muncul di capture; bandingkan `Stamp` yang dikirim mesin dengan `ATTLOGStamp`/`Stamp` di handshake berikutnya | setelah server menyimpan stamp, mesin hanya mengirim log yang lebih baru (hipotesis `Resend::FromStamp`) | bila mesin tetap mengirim ulang seluruh batch (`Resend::All`) itu tidak merusak data (idempoten) tetapi menambah beban; catat dan putuskan apakah perlu mengubah format stamp |
| 4 | **Mesin tak terdaftar menyimpan datanya?** | mulai dengan mesin **belum** didaftarkan; lakukan 2-3 absensi; daftarkan mesin; amati apakah log lama tiba | data tiba setelah terdaftar (keputusan D2, balasan kosong) | bila log lama tidak pernah tiba, firmware membuang data pada balasan non-OK: ubah ke balasan `OK` dan simpan log mentah dari mesin tak terdaftar untuk ditinjau |
| 5 | **Jam dan zona** | bandingkan waktu absensi di mesin, `device_logs.device_time`, dan `punched_at`; periksa `TimeZone=` di handshake | `punched_at` = waktu lokal mesin dikonversi ke UTC sesuai `devices.timezone` | samakan zona di mesin dan v2; bila `TimeZone` (jam desimal) ditafsirkan lain oleh firmware, sesuaikan `zk_time::hours_label` |
| 6 | **`SET TIME`** (dua baris, id sama) | antrekan "Atur jam mesin" di UI v2; capture balasan `getrequest` dan `devicecmd` | dua baris `C:<id>:SET OPTIONS ...`; mesin menjawab keduanya dengan `Return=0`; jam mesin berubah | bila mesin menolak atau hanya menjawab sekali, pecah menjadi dua jenis perintah atau satu baris `SET OPTIONS MachineTZ=...,DateTime=...` (lihat implementation-notes Task 4) |
| 7 | **`REBOOT`, `INFO`, `CHECK`** | antrekan dari UI v2 | mesin menjalankan dan menjawab `Return=0`; kolom info mesin terisi | catat `Return` dan `CMD` aktual |
| 8 | **`ADD USER`** | kirim satu pegawai | pegawai muncul di mesin; `Return=0` | catat panjang nama maksimum, karakter yang ditolak, dan format `DATA USER`/`DATA UPDATE USERINFO` yang diterima firmware |
| 9 | **PIN** | pakai PIN terpanjang yang direncanakan | diterima dan cocok dengan `employee_number` | catat batas PIN firmware; batas validasi v2 (`ATTENDANCE_DEVICE_PIN_MAX_LENGTH`) harus sama atau lebih ketat |
| 10 | **Sidik jari: unggah** | daftarkan satu jari di mesin; tunggu mesin mengirim OPERLOG; `SELECT kind, finger_index, algorithm_version, key_id FROM biometric_templates` | satu baris terenkripsi (`key_id` = kunci terkini) | capture baris `FP`/`FACE`; bila bentuknya berbeda (mis. tabel `BIODATA` pada firmware baru, yang belum diterima) ubah `protocol::biometric` |
| 11 | **Sidik jari: kirim ke mesin lain** | dengan dua mesin, antrekan "Kirim sidik jari" | mesin kedua menerima (`Return=0`) dan mengenali jari itu | bila `Size`/`TMP` ditolak, bandingkan dengan format yang dikirim mesin pertama |
| 12 | **`rtlog`/`Realtime=1`** | capture apakah mesin memakai `table=rtlog` | tidak dipakai atau diabaikan tanpa efek (keputusan D7) | bila log absensi hanya datang lewat `rtlog` pada mesin tertentu, aktifkan pemrosesan `rtlog` |
| 13 | **Pemulihan jaringan** | putus lalu sambung jaringan mesin saat ada absensi | semua log tiba tanpa duplikat; `check-offline` melaporkan lalu memulihkan | catat durasi sampai mesin kembali online |

## Hasil pertama

Lihat bagian "Temuan uji mesin fisik pertama" di [implementation-notes.md](implementation-notes.md): model `UA200`, firmware `Ver 6.60 Jul 24 2013`; handshake, polling, INFO/CHECK/REBOOT, dan penerimaan ATTLOG terbukti; `set_time` tidak dijawab, `Stamp` tidak terkirim, dan sinkronisasi jam lewat header `Date` sedang diuji.

## Hasil
Isi tabel ini (atau salin ke tiket) setelah uji pertama, lalu perbarui [legacy-analysis.md](legacy-analysis.md) bagian 10 dan [implementation-notes.md](implementation-notes.md) dengan fakta yang terbukti.

| Model dan firmware | `pushver` / `DeviceType` | Tanggal uji | Nomor yang gagal dan catatannya |
|---|---|---|---|
| | | | |

Setelah satu model lulus, simpan contoh payload asli (PIN disamarkan) sebagai fixture baru di `tests/fixtures/` dan tambahkan ke uji golden sehingga perilaku itu tidak berubah tanpa disadari.
