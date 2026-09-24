use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use anyhow::{anyhow, Result};

pub struct HttpResponse {
    pub status: u16,
    /// Reason phrase from the status line (e.g. "Bad Request"), empty when absent.
    pub reason: String,
    pub body: String,
    /// `Retry-After` header in seconds, when present and numeric.
    pub retry_after: Option<u64>,
}

impl HttpResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

fn build_headers(host: &str, port: u16, token: Option<&str>, content_type: Option<&str>, body_len: Option<usize>) -> String {
    let mut headers = format!("Host: {host}:{port}\r\nAccept: */*\r\nConnection: close\r\n");
    if let Some(tok) = token {
        headers.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    }
    if let Some(ct) = content_type {
        headers.push_str(&format!("Content-Type: {ct}\r\n"));
    }
    if let Some(len) = body_len {
        headers.push_str(&format!("Content-Length: {len}\r\n"));
    }
    headers
}

fn send_request(
    port: u16,
    request_line: &str,
    token: Option<&str>,
    body: Option<&[u8]>,
    content_type: Option<&str>,
    timeout: Duration,
) -> Result<HttpResponse> {
    let host = "127.0.0.1";
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| anyhow!("connect 127.0.0.1:{port} failed: {e}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();

    let mut req = String::new();
    req.push_str(request_line);
    req.push_str("\r\n");
    req.push_str(&build_headers(
        host,
        port,
        token,
        content_type,
        body.map(|b| b.len()),
    ));
    req.push_str("\r\n");
    stream.write_all(req.as_bytes())?;
    if let Some(b) = body {
        stream.write_all(b)?;
    }
    stream.flush().ok();

    // Read entire response until EOF.
    let mut raw = Vec::with_capacity(1024);
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) => return Err(anyhow!("read failed: {e}")),
        }
    }

    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response (no header terminator)"))?;
    let header_bytes = &raw[..split];
    let body_bytes = &raw[split + 4..];

    let head = std::str::from_utf8(header_bytes).unwrap_or("");
    let first_line = head.lines().next().unwrap_or("");
    let mut parts = first_line.splitn(3, ' ');
    let _ = parts.next();
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("invalid HTTP status line: {first_line}"))?;
    let reason = parts.next().unwrap_or("").trim().to_owned();

    // Very small subset of chunked transfer decoding — Unity's Mono
    // HttpListener always emits Content-Length, but be defensive.
    let is_chunked = head.to_ascii_lowercase().contains("transfer-encoding: chunked");
    let body = if is_chunked {
        decode_chunked(body_bytes)?
    } else {
        String::from_utf8_lossy(body_bytes).into_owned()
    };

    Ok(HttpResponse {
        status,
        reason,
        body,
        retry_after: parse_retry_after(head),
    })
}

/// Extract `Retry-After` (delta-seconds form only) from the response headers.
fn parse_retry_after(head: &str) -> Option<u64> {
    head.lines()
        .skip(1)
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("retry-after") {
                value.trim().parse::<u64>().ok()
            } else {
                None
            }
        })
}

fn decode_chunked(data: &[u8]) -> Result<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        // Chunk size line.
        let line_end = data[i..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| anyhow!("malformed chunked body"))?;
        let size_str = std::str::from_utf8(&data[i..i + line_end])
            .map_err(|_| anyhow!("chunk size is not utf-8"))?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_str, 16)
            .map_err(|_| anyhow!("invalid chunk size: {size_str}"))?;
        i += line_end + 2;
        if size == 0 {
            break;
        }
        if i + size > data.len() {
            return Err(anyhow!("truncated chunk"));
        }
        out.extend_from_slice(&data[i..i + size]);
        i += size + 2;
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Simple HTTP GET against the local Pipeline server.
pub fn http_get(port: u16, path: &str, token: Option<&str>, timeout: Duration) -> Result<HttpResponse> {
    send_request(port, &format!("GET {path} HTTP/1.1"), token, None, None, timeout)
}

/// Simple HTTP POST with a JSON body.
pub fn http_post_json(
    port: u16,
    path: &str,
    token: Option<&str>,
    body: &str,
    timeout: Duration,
) -> Result<HttpResponse> {
    send_request(
        port,
        &format!("POST {path} HTTP/1.1"),
        token,
        Some(body.as_bytes()),
        Some("application/json"),
        timeout,
    )
}

/// Probe whether the local Pipeline server responds on 127.0.0.1. Returns true
/// when the endpoint answers with any 2xx status, or with 401 when no bearer
/// token is supplied (an unauthenticated 401 still proves the server is alive).
pub fn probe_pipeline(port: u16, token: Option<&str>) -> bool {
    match http_get(port, "/api/status", token, Duration::from_millis(1500)) {
        Ok(resp) if resp.ok() => true,
        Ok(resp) if resp.status == 401 && token.is_none() => true,
        _ => false,
    }
}
