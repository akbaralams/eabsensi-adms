# Uji beban

Dijalankan 25 Oktober 2026 dengan `cargo test --test simulator_load -- --nocapture` (uji `seratus_mesin_mengirim_log_dan_polling_bersamaan`, ikut berjalan di CI). Angka di bawah dari mesin pengembangan, **bukan** lingkungan produksi: server ADMS, simulator, dan PostgreSQL 18 berjalan di satu laptop Windows (loopback, tanpa latensi jaringan, CPU dan disk dibagi dengan alat lain).

## Cara kerja
- 100 mesin simulasi (`src/simulator`) membuka koneksi TCP sungguhan, satu koneksi per request seperti firmware (`Connection: close`, `Content-Type` kosong).
- Server memakai kolam database **10 koneksi** (bawaan `ADMS_DB_MAX_CONNECTIONS`), batas body 8 MiB, timeout 30 detik.
- Tidak ada jeda antar request. Mesin asli polling tiap ±60 detik, jadi uji ini menekan jauh lebih keras daripada beban nyata.
- Fase 1 "siklus penuh": tiap mesin 15 siklus. Satu siklus = handshake, unggah ulang seluruh ingatannya (20 log, perilaku yang tercatat di log produksi lama), polling, dan hasil perintah. Siklus pertama menyertakan INFO dan CHECK.
- Fase 2 "polling": tiap mesin 30 kali `getrequest`.
- Sesudahnya diperiksa bahwa database berisi tepat 2.000 log dan 2.000 punch (100 mesin × 20 log) walau tiap log dikirim 15 kali.

## Hasil (jalan terakhir; tiga jalan sebelumnya mirip)
| Fase | Request | Galat | Durasi | Request/detik | p50 | p95 | p99 | Maks |
|---|---|---|---|---|---|---|---|---|
| Siklus penuh | 4.700 | 0 | 11,1 dtk | 425 | 227 ms | 321 ms | 915 ms | 1,15 dtk |
| Polling saja | 3.000 | 0 | 6,5 dtk | 459 | 215 ms | 251 ms | 283 ms | 301 ms |

Pengukuran lain di mesin yang sama:

| Uji | Hasil |
|---|---|
| 2.000 baris ATTLOG baru, satu request, langsung ke router | ±2,3 dtk; kiriman ulang batch yang sama ±26 ms |
| 5.000 baris ATTLOG baru lewat HTTP (5 request × 1.000 baris) | ±6-7 dtk; kiriman ulang ±108 ms |

## Cara membaca
- Latensi p50 ≈ 220 ms pada fase polling adalah antrean, bukan biaya satu request: 100 klien tanpa jeda dibagi ±460 request/detik (hukum Little). Satu `getrequest` melakukan 4 query (cari mesin, heartbeat, penyegaran perintah tanpa jawaban, pengambilan perintah) lewat kolam 10 koneksi.
- Beban nyata 100 mesin dengan jeda 60 detik ≈ 1,7 request/detik, kira-kira 0,4% dari kapasitas yang terukur di sini. Ekstrapolasi kasar: ribuan mesin masih di dalam kapasitas satu proses, tetapi itu belum diuji dan bergantung pada database, jaringan, dan perangkat keras sungguhan.
- Penerimaan batch baru ≈ 1,2-1,4 ms per baris (dua `INSERT` per baris dalam satu transaksi); kiriman ulang praktis gratis karena disaring dengan satu query. Batch 1.000 baris ≈ 1,2 dtk, jauh di bawah timeout 30 detik.
- Kemungkinan optimasi bila kelak perlu (belum dilakukan, tanpa bukti kebutuhan): menggabungkan heartbeat dan penyegaran perintah ke satu query, dan men-throttle `UPDATE devices SET last_seen_at` bila polling sangat rapat.

## Batas yang ditegakkan di uji
Tanpa galat jaringan atau siklus gagal, log dan punch masuk tepat sekali, dan p99 < 5 detik di kedua fase. Batas sengaja longgar agar tidak rapuh di runner CI yang lambat.

## Menjalankan beban sendiri
```bash
cargo run --bin adms-sim -- --url http://127.0.0.1:8090 --serial LOAD --machines 100 --cycles 20 --rows 20 --pin <employee_number>
```
Mesin `LOAD-0000`…`LOAD-0099` harus sudah terdaftar di tabel `devices` (keputusan D2: mesin tak terdaftar ditolak) dan `--pin` harus `employee_number` pegawai yang ada agar log menjadi punch.
