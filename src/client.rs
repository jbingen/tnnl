use std::time::Duration;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures::future::poll_fn;
use futures::AsyncWriteExt;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use yamux::{Config, Connection, Mode};

use crate::log as tlog;
use crate::protocol::{self, ControlMsg};
use crate::proxy;
use crate::store;

const MAX_BACKOFF: Duration = Duration::from_secs(30);
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const BODY_CAP: usize = 10 * 1024 * 1024; // 10 MB inspect cap

pub async fn run(
    local_port: u16,
    server_addr: &str,
    server_port: u16,
    token: &str,
    subdomain: Option<&str>,
    auth: Option<&str>,
    inspect: bool,
) -> Result<()> {
    store::init();
    // Precompute "Basic <b64>" once so we're not doing it per-request.
    let expected_auth = auth.map(|a| format!("Basic {}", B64.encode(a)));
    let mut backoff = INITIAL_BACKOFF;

    loop {
        tlog::info(&format!("connecting to {server_addr}:{server_port}..."));

        match connect_and_tunnel(local_port, server_addr, server_port, token, subdomain, expected_auth.as_deref(), inspect).await
        {
            Ok(()) => {
                tlog::info("connection closed");
                break;
            }
            Err(e) => {
                tlog::error(&format!("{e:#}"));
                tlog::info(&format!("reconnecting in {}s...", backoff.as_secs()));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }

    Ok(())
}

async fn connect_and_tunnel(
    local_port: u16,
    server_addr: &str,
    server_port: u16,
    token: &str,
    subdomain: Option<&str>,
    expected_auth: Option<&str>,
    inspect: bool,
) -> Result<()> {
    let socket = TcpStream::connect(format!("{server_addr}:{server_port}"))
        .await
        .context("failed to connect to server")?;

    let mut config = Config::default();
    config.set_split_send_size(16 * 1024);

    let mut connection = Connection::new(socket.compat(), config, Mode::Client);

    let mut control_stream = poll_fn(|cx| connection.poll_new_outbound(cx))
        .await
        .context("failed to open control stream")?;

    let (inbound_tx, mut inbound_rx) = tokio::sync::mpsc::channel::<yamux::Stream>(32);
    tokio::spawn(async move {
        loop {
            match poll_fn(|cx| connection.poll_next_inbound(cx)).await {
                Some(Ok(stream)) => {
                    if inbound_tx.send(stream).await.is_err() {
                        break;
                    }
                }
                Some(Err(e)) => {
                    tlog::error(&format!("yamux: {e}"));
                    break;
                }
                None => break,
            }
        }
    });

    let nonce = {
        use rand::Rng;
        let bytes: [u8; 32] = rand::rng().random();
        B64.encode(bytes)
    };
    let hmac = if token.is_empty() {
        None
    } else {
        Some(protocol::compute_hmac(token, &nonce))
    };
    let auth = ControlMsg::Auth {
        subdomain: subdomain.map(|s| s.to_string()),
        nonce,
        hmac,
    };
    control_stream.write_all(&auth.encode()?).await?;
    control_stream.flush().await?;

    let resp = protocol::read_msg(&mut control_stream).await?;
    let tunnel_url = match resp {
        ControlMsg::AuthOk { url, .. } => url,
        ControlMsg::Error { message } => anyhow::bail!("server error: {message}"),
        _ => anyhow::bail!("unexpected response from server"),
    };

    tlog::banner(&tunnel_url, local_port, inspect);

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        eprintln!();
        tlog::info("shutting down...");
        let _ = shutdown_tx.send(());
    });

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown_rx => {
                control_stream.close().await.ok();
                break;
            }
            stream = inbound_rx.recv() => {
                match stream {
                    Some(s) => { tokio::spawn(handle_stream(s, local_port, expected_auth.map(|s| s.to_string()), inspect)); }
                    None => break,
                }
            }
        }
    }

    Ok(())
}

async fn handle_stream(stream: yamux::Stream, local_port: u16, expected_auth: Option<String>, inspect: bool) {
    if let Err(e) = proxy_to_localhost(stream, local_port, expected_auth.as_deref(), inspect).await {
        tlog::error(&format!("proxy: {e}"));
    }
}

async fn proxy_to_localhost(
    stream: yamux::Stream,
    local_port: u16,
    expected_auth: Option<&str>,
    inspect: bool,
) -> Result<()> {
    let mut tunnel = stream.compat();

    let req_head = proxy::read_http_head(&mut tunnel).await?;
    let (method, path) = proxy::parse_request_line(&req_head);
    let start = std::time::Instant::now();
    let id = store::next_id();

    let head_end = proxy::headers_end(&req_head).unwrap_or(req_head.len());
    let req_headers = &req_head[..head_end];
    let body_prefix = req_head[head_end..].to_vec();
    let content_length = proxy::parse_content_length(req_headers);

    if let Some(expected) = expected_auth {
        let provided = proxy::extract_authorization(req_headers);
        if provided.as_deref() != Some(expected) {
            proxy::write_401(&mut tunnel).await.ok();
            tlog::request(&method, &path, 401, start.elapsed().as_millis() as u64, id);
            return Ok(());
        }
    }

    let mut local = match TcpStream::connect(format!("127.0.0.1:{local_port}")).await {
        Ok(s) => s,
        Err(_) => {
            proxy::write_502(&mut tunnel).await.ok();
            tlog::request(&method, &path, 502, start.elapsed().as_millis() as u64, id);
            return Ok(());
        }
    };

    if inspect {
        // Inspect path: buffer full request body before forwarding so we can display it.
        let req_body = read_body_exact(&mut tunnel, body_prefix, content_length).await;

        local.write_all(req_headers).await?;
        local.write_all(&req_body).await?;
        local.flush().await?;

        store::store(store::StoredRequest {
            id,
            port: local_port,
            method: method.clone(),
            path: path.clone(),
            raw_headers: String::from_utf8_lossy(req_headers).into_owned(),
            body_b64: B64.encode(&req_body),
        });

        // Read and buffer complete response.
        let resp_head = proxy::read_http_head(&mut local).await.unwrap_or_default();
        let status = proxy::parse_response_status(&resp_head);
        let resp_head_end = proxy::headers_end(&resp_head).unwrap_or(resp_head.len());
        let resp_headers = &resp_head[..resp_head_end];
        let resp_already = resp_head[resp_head_end..].to_vec();

        let resp_cl = proxy::parse_content_length(resp_headers);
        let resp_body = if resp_cl > 0 {
            read_body_exact(&mut local, resp_already, resp_cl).await
        } else if proxy::is_chunked(resp_headers) {
            read_chunked(&mut local, resp_already).await
        } else {
            resp_already
        };

        // Unchunk the response so the forwarded bytes are valid HTTP.
        let out_headers = if proxy::is_chunked(resp_headers) {
            rebuild_resp_headers(resp_headers, resp_body.len())
        } else {
            resp_headers.to_vec()
        };

        tunnel.write_all(&out_headers).await?;
        tunnel.write_all(&resp_body).await?;
        tunnel.flush().await.ok();

        let elapsed = start.elapsed().as_millis() as u64;
        tlog::request(&method, &path, status, elapsed, id);

        let req_raw = String::from_utf8_lossy(req_headers);
        let req_body_str = String::from_utf8_lossy(&req_body);
        tlog::inspect_request(id, &req_raw, &req_body_str);

        let resp_raw = String::from_utf8_lossy(&out_headers);
        let resp_body_str = String::from_utf8_lossy(&resp_body);
        tlog::inspect_response(status, &resp_raw, &resp_body_str, id);
    } else {
        // Transparent path: forward headers + already-buffered prefix immediately,
        // then stream the rest bidirectionally. Body is never fully buffered.
        local.write_all(req_headers).await?;
        local.write_all(&body_prefix).await?;
        local.flush().await?;

        // Store what we have (headers + prefix). For requests without a body or
        // where the body fit in the initial read, this is complete.
        store::store(store::StoredRequest {
            id,
            port: local_port,
            method: method.clone(),
            path: path.clone(),
            raw_headers: String::from_utf8_lossy(req_headers).into_owned(),
            body_b64: B64.encode(&body_prefix),
        });

        // Peek first response chunk for status, then stream the rest.
        let mut peek = [0u8; 512];
        let n = local.read(&mut peek).await.unwrap_or(0);
        let status = proxy::parse_response_status(&peek[..n]);
        tunnel.write_all(&peek[..n]).await?;
        tokio::io::copy_bidirectional(&mut local, &mut tunnel).await.ok();
        tlog::request(&method, &path, status, start.elapsed().as_millis() as u64, id);
    }

    Ok(())
}

/// Loop-read until we have `total` bytes or EOF, capped at BODY_CAP.
async fn read_body_exact<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    mut buf: Vec<u8>,
    total: usize,
) -> Vec<u8> {
    let target = total.min(BODY_CAP);
    let mut tmp = [0u8; 8192];
    while buf.len() < target {
        let want = (target - buf.len()).min(8192);
        match reader.read(&mut tmp[..want]).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    buf
}

/// Decode an HTTP chunked body into raw bytes.
async fn read_chunked<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    initial: Vec<u8>,
) -> Vec<u8> {
    let mut raw = initial;
    let mut body = Vec::new();
    let mut tmp = [0u8; 8192];

    'outer: loop {
        let mut pos = 0;
        loop {
            let slice = &raw[pos..];
            let Some(crlf) = slice.windows(2).position(|w| w == b"\r\n") else {
                break;
            };
            let size_str = std::str::from_utf8(&slice[..crlf])
                .unwrap_or("0")
                .split(';')
                .next()
                .unwrap_or("0")
                .trim();
            let chunk_size = usize::from_str_radix(size_str, 16).unwrap_or(0);
            if chunk_size == 0 {
                // Terminal chunk: "0\r\n\r\n". We've already consumed the first
                // \r\n (the size line ending). We need one more \r\n to follow.
                // Only exit if that trailing \r\n is also present in the buffer.
                let after_size_line = pos + crlf + 2;
                if after_size_line + 2 <= raw.len() {
                    break 'outer;
                }
                // Not enough data yet — fall through to read more.
                break;
            }
            let data_start = pos + crlf + 2;
            let data_end = data_start + chunk_size;
            if data_end + 2 > raw.len() {
                break;
            }
            body.extend_from_slice(&raw[data_start..data_end]);
            pos = data_end + 2;
            if body.len() >= BODY_CAP {
                break 'outer;
            }
        }
        raw.drain(..pos);
        match reader.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => raw.extend_from_slice(&tmp[..n]),
        }
    }
    body
}

/// Strip Transfer-Encoding and insert Content-Length so the unchunked body is valid.
fn rebuild_resp_headers(headers: &[u8], body_len: usize) -> Vec<u8> {
    let text = String::from_utf8_lossy(headers);
    let mut out = Vec::new();
    for (i, line) in text.split("\r\n").enumerate() {
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("transfer-encoding:") || lower.starts_with("content-length:") {
            continue;
        }
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(b"\r\n");
        if i == 0 {
            out.extend_from_slice(format!("Content-Length: {body_len}\r\n").as_bytes());
        }
    }
    out.extend_from_slice(b"\r\n");
    out
}

pub async fn replay(id: u64) -> Result<()> {
    let req = store::find(id)
        .ok_or_else(|| anyhow::anyhow!("request #{id} not found"))?;

    tlog::info(&format!("replaying #{id}: {} {}", req.method, req.path));

    let mut local = TcpStream::connect(format!("127.0.0.1:{}", req.port))
        .await
        .with_context(|| format!("failed to connect to localhost:{}", req.port))?;

    local.write_all(req.raw_headers.as_bytes()).await?;
    let body = B64.decode(&req.body_b64).unwrap_or_default();
    local.write_all(&body).await?;
    local.flush().await?;

    let resp_head = proxy::read_http_head(&mut local).await.unwrap_or_default();
    let resp_head_end = proxy::headers_end(&resp_head).unwrap_or(resp_head.len());
    let resp_headers = &resp_head[..resp_head_end];
    let resp_already = resp_head[resp_head_end..].to_vec();

    let resp_cl = proxy::parse_content_length(resp_headers);
    let resp_body = if resp_cl > 0 {
        read_body_exact(&mut local, resp_already, resp_cl).await
    } else if proxy::is_chunked(resp_headers) {
        read_chunked(&mut local, resp_already).await
    } else {
        resp_already
    };

    println!("{}", String::from_utf8_lossy(&resp_body));

    tlog::success(&format!("replayed #{id}"));
    Ok(())
}
