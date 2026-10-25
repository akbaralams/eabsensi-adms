//! Klien HTTP mentah yang meniru perilaku firmware ZKTeco: HTTP/1.1, satu koneksi per request
//! (`Connection: close`), `Content-Type` kosong, tanpa kompresi. Dibuat sendiri (bukan pustaka
//! klien) agar bentuk request persis seperti yang tertangkap di log produksi lama.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::SimError;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    /// `host:port` tujuan (juga dipakai sebagai header `Host`).
    authority: String,
    user_agent: String,
}

impl Client {
    pub fn new(authority: impl Into<String>, user_agent: impl Into<String>) -> Self {
        Self {
            authority: authority.into(),
            user_agent: user_agent.into(),
        }
    }

    pub async fn get(&self, target: &str) -> Result<Response, SimError> {
        self.request("GET", target, None).await
    }

    pub async fn post(&self, target: &str, body: &[u8]) -> Result<Response, SimError> {
        self.request("POST", target, Some(body)).await
    }

    async fn request(
        &self,
        method: &str,
        target: &str,
        body: Option<&[u8]>,
    ) -> Result<Response, SimError> {
        timeout(REQUEST_TIMEOUT, self.exchange(method, target, body))
            .await
            .map_err(|_| SimError::Timeout)?
    }

    async fn exchange(
        &self,
        method: &str,
        target: &str,
        body: Option<&[u8]>,
    ) -> Result<Response, SimError> {
        let mut stream = TcpStream::connect(&self.authority).await?;

        // Urutan dan isi header sama dengan request mesin di log produksi lama.
        let mut head = format!(
            "{method} {target} HTTP/1.1\r\nAccept: */*\r\nConnection: close\r\nContent-Type: \r\nHost: {}\r\nUser-Agent: {}\r\n",
            self.authority, self.user_agent
        );
        if let Some(body) = body {
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");

        stream.write_all(head.as_bytes()).await?;
        if let Some(body) = body {
            stream.write_all(body).await?;
        }
        stream.flush().await?;

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await?;
        parse_response(&raw)
    }
}

fn parse_response(raw: &[u8]) -> Result<Response, SimError> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| SimError::BadResponse("respons tanpa akhir header".into()))?;
    let head = std::str::from_utf8(&raw[..split])
        .map_err(|_| SimError::BadResponse("header bukan UTF-8".into()))?;
    let body = &raw[split + 4..];

    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| SimError::BadResponse("baris status tidak terbaca".into()))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect();

    let chunked = headers.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("transfer-encoding") && value.eq_ignore_ascii_case("chunked")
    });
    let body = if chunked {
        decode_chunked(body)?
    } else {
        body.to_vec()
    };

    Ok(Response {
        status,
        headers,
        body,
    })
}

fn decode_chunked(mut data: &[u8]) -> Result<Vec<u8>, SimError> {
    let bad = || SimError::BadResponse("badan chunked rusak".into());
    let mut out = Vec::new();
    loop {
        let line_end = data
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(bad)?;
        let size_text = std::str::from_utf8(&data[..line_end]).map_err(|_| bad())?;
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| bad())?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size + 2 {
            return Err(bad());
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respons_dengan_content_length() {
        let response = parse_response(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=UTF-8\r\ncontent-length: 2\r\n\r\nOK",
        )
        .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.text(), "OK");
        assert_eq!(
            response.header("Content-Type"),
            Some("text/plain; charset=UTF-8")
        );
    }

    #[test]
    fn respons_kosong_dan_chunked() {
        let empty = parse_response(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").unwrap();
        assert_eq!(empty.body, b"");

        let chunked = parse_response(
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(chunked.text(), "abcde");
    }

    #[test]
    fn respons_rusak_ditolak() {
        assert!(parse_response(b"bukan http").is_err());
        assert!(parse_response(b"HTTP/1.1 abc\r\n\r\n").is_err());
        assert!(
            parse_response(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\nzz\r\n").is_err()
        );
    }
}
