//! Bentuk respons di kabel (nama header berhuruf kapital awal) dan berhenti dengan rapi.
//! Butuh `DATABASE_URL` hanya karena memakai router aplikasi yang sama dengan produksi.

mod common;

use std::time::Duration;

use common::{app, default_queue, pool};
use eabsensi_adms::http;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Request mentah ala mesin, hasilnya teks respons utuh (sampai koneksi ditutup).
async fn raw(addr: std::net::SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.unwrap();
    String::from_utf8(out).unwrap()
}

const PING: &str = "GET /iclock/ping HTTP/1.1\r\nAccept: */*\r\nConnection: close\r\nContent-Type: \r\nHost: x\r\nUser-Agent: iClock Proxy/1.09\r\n\r\n";

#[tokio::test]
async fn nama_header_respons_berhuruf_kapital_awal_dan_ada_date() {
    let pool = pool().await;
    let server = common::spawn(app(&pool, default_queue())).await;

    let response = raw(server.addr, PING).await;

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.contains("\r\nContent-Type: text/plain; charset=UTF-8\r\n"),
        "{response}"
    );
    assert!(response.contains("\r\nContent-Length: 2\r\n"), "{response}");
    assert!(response.contains("\r\nDate: "), "{response}");
    assert!(response.ends_with("\r\n\r\nOK"), "{response}");
    // Tidak ada nama header huruf kecil bawaan hyper.
    for lower in ["content-type:", "content-length:", "date:", "connection:"] {
        assert!(
            !response.contains(&format!("\r\n{lower}")),
            "{lower} masih huruf kecil: {response}"
        );
    }
}

#[tokio::test]
async fn date_berformat_http_dan_sesuai_jam_server() {
    let pool = pool().await;
    let server = common::spawn(app(&pool, default_queue())).await;

    let response = raw(server.addr, PING).await;
    let date = response
        .lines()
        .find_map(|line| line.strip_prefix("Date: "))
        .expect("header Date");

    // Contoh: "Mon, 26 Oct 2026 13:28:57 GMT"
    let parsed = chrono::DateTime::parse_from_rfc2822(date).expect("RFC 2822");
    let drift = (chrono::Utc::now() - parsed.with_timezone(&chrono::Utc))
        .num_seconds()
        .abs();
    assert!(date.ends_with("GMT"));
    assert!(drift <= 5, "Date menyimpang {drift} detik dari jam server");
}

#[tokio::test]
async fn berhenti_rapi_menunggu_request_berjalan_dan_menolak_yang_baru() {
    let pool = pool().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let router = app(&pool, default_queue());
    let task = tokio::spawn(async move {
        http::serve(listener, router, async {
            let _ = stopped.await;
        })
        .await
    });

    // Sebelum berhenti: melayani seperti biasa.
    assert!(raw(addr, PING).await.ends_with("OK"));

    stop.send(()).unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(5), task).await;
    assert!(finished.is_ok(), "server harus berhenti dalam 5 detik");
    assert!(finished.unwrap().unwrap().is_ok());

    // Setelah berhenti, port tidak lagi menerima koneksi.
    assert!(TcpStream::connect(addr).await.is_err());
}
