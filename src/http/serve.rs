//! Penyaji HTTP/1.1 di atas hyper (bukan `axum::serve`) supaya bentuk respons di kabel sama dengan
//! yang dikirim server lama: nama header berhuruf kapital awal (`Content-Type`, `Content-Length`,
//! `Date`) alih-alih huruf kecil bawaan hyper.
//!
//! Alasannya firmware: mesin ZKTeco tua mengurai respons dengan pengurai sederhana, dan di
//! acuan lama mesin yang tidak pernah meminta `push`/`rtdata` menyelaraskan jamnya dari header
//! `Date` respons. Pengurai yang peka huruf besar/kecil tidak akan menemukan `date:`. Ini
//! hipotesis yang belum terbukti pada mesin fisik; biayanya nol karena header HTTP tidak peka huruf
//! besar/kecil bagi klien yang benar.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tower::Service;

/// Lama menunggu koneksi yang sedang berjalan selesai setelah sinyal berhenti.
const DRAIN: Duration = Duration::from_secs(30);

/// Melayani `app` di `listener` sampai `shutdown` selesai, lalu menunggu request yang sedang
/// berjalan (maksimal 30 detik). Alamat klien tersedia bagi handler lewat
/// `ConnectInfo<SocketAddr>`, seperti pada `axum::serve`.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    let graceful = GracefulShutdown::new();
    tokio::pin!(shutdown);

    loop {
        let (stream, remote) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(error) => {
                    tracing::warn!(%error, "gagal menerima koneksi");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };

        // Request mesin kecil dan sering; jangan menunggu paket berikutnya.
        let _ = stream.set_nodelay(true);

        let app = app.clone();
        let service = service_fn(move |mut request: hyper::Request<Incoming>| {
            request
                .extensions_mut()
                .insert(ConnectInfo::<SocketAddr>(remote));
            let mut app = app.clone();
            async move { app.call(request).await }
        });

        let connection = http1::Builder::new()
            .title_case_headers(true)
            .timer(TokioTimer::new())
            .serve_connection(TokioIo::new(stream), service);
        let connection = graceful.watch(connection);
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::debug!(%error, "koneksi berakhir dengan galat");
            }
        });
    }

    // Berhenti menerima koneksi baru, lalu tunggu yang berjalan.
    drop(listener);
    if tokio::time::timeout(DRAIN, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("koneksi yang tersisa dihentikan setelah batas waktu 30 detik");
    }
    Ok(())
}
