use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use dashmap::DashMap;
use futures::future::poll_fn;
use futures::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use yamux::{Config, Connection, Mode};

use crate::log as tlog;
use crate::protocol::{self, ControlMsg};
use crate::proxy;

const MAX_TUNNELS_TOTAL: usize = 200;
const MAX_TUNNELS_PER_IP: usize = 5;
const MAX_CONNECTS_PER_MINUTE: usize = 15;
const RATE_WINDOW: Duration = Duration::from_secs(60);

type OpenStreamReply = oneshot::Sender<Result<yamux::Stream>>;

struct ClientHandle {
    stream_tx: mpsc::Sender<OpenStreamReply>,
}

type Registry = Arc<DashMap<String, ClientHandle>>;

struct IpState {
    recent_connects: VecDeque<Instant>,
    active_tunnels: usize,
}

type IpTracker = Arc<DashMap<IpAddr, IpState>>;

pub async fn run(
    control_port: u16,
    http_port: u16,
    domain: &str,
    token: Option<&str>,
) -> Result<()> {
    let registry: Registry = Arc::new(DashMap::new());
    let ip_tracker: IpTracker = Arc::new(DashMap::new());
    let domain = domain.to_string();
    let token = token.map(|t| t.to_string());

    let control_listener = TcpListener::bind(format!("0.0.0.0:{control_port}"))
        .await
        .context(format!("failed to bind control port {control_port}"))?;
    let http_listener = TcpListener::bind(format!("0.0.0.0:{http_port}"))
        .await
        .context(format!("failed to bind http port {http_port}"))?;

    tlog::info(&format!("control listening on 0.0.0.0:{control_port}"));
    tlog::info(&format!("http listening on 0.0.0.0:{http_port}"));
    tlog::info(&format!("domain: *.{domain}"));
    if token.is_none() {
        tlog::info("token auth disabled — open server");
    }

    let reg = registry.clone();
    let ipt = ip_tracker.clone();
    let tok = token.clone();
    let dom = domain.clone();
    let control_task = tokio::spawn(async move {
        loop {
            match control_listener.accept().await {
                Ok((socket, addr)) => {
                    let ip = addr.ip();

                    if !check_rate_limit(&ipt, ip) {
                        tlog::info(&format!(
                            "rate limited {ip} (>{MAX_CONNECTS_PER_MINUTE}/min)"
                        ));
                        drop(socket);
                        continue;
                    }

                    tlog::info(&format!("client connected from {addr}"));
                    let reg = reg.clone();
                    let ipt = ipt.clone();
                    let tok = tok.clone();
                    let dom = dom.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            handle_client(socket, reg, ipt, ip, tok.as_deref(), &dom).await
                        {
                            tlog::error(&format!("client {addr}: {e}"));
                        }
                    });
                }
                Err(e) => tlog::error(&format!("accept error: {e}")),
            }
        }
    });

    let reg = registry.clone();
    let dom = domain.clone();
    let http_task = tokio::spawn(async move {
        loop {
            match http_listener.accept().await {
                Ok((socket, _)) => {
                    let reg = reg.clone();
                    let dom = dom.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_http(socket, reg, &dom).await {
                            tlog::error(&format!("http: {e}"));
                        }
                    });
                }
                Err(e) => tlog::error(&format!("http accept error: {e}")),
            }
        }
    });

    tokio::select! {
        r = control_task => r?,
        r = http_task => r?,
    }

    Ok(())
}

// Returns false if the IP has exceeded the connection rate limit.
// Also prunes stale timestamps on every call.
fn check_rate_limit(ip_tracker: &IpTracker, ip: IpAddr) -> bool {
    let now = Instant::now();
    let cutoff = now - RATE_WINDOW;
    let mut entry = ip_tracker.entry(ip).or_insert_with(|| IpState {
        recent_connects: VecDeque::new(),
        active_tunnels: 0,
    });
    while entry.recent_connects.front().is_some_and(|t| *t < cutoff) {
        entry.recent_connects.pop_front();
    }
    if entry.recent_connects.len() >= MAX_CONNECTS_PER_MINUTE {
        return false;
    }
    entry.recent_connects.push_back(now);
    true
}

async fn handle_client(
    socket: tokio::net::TcpStream,
    registry: Registry,
    ip_tracker: IpTracker,
    peer_ip: IpAddr,
    expected_token: Option<&str>,
    domain: &str,
) -> Result<()> {
    let mut config = Config::default();
    config.set_split_send_size(16 * 1024);

    let mut connection = Connection::new(socket.compat(), config, Mode::Server);

    let mut control_stream = poll_fn(|cx| connection.poll_next_inbound(cx))
        .await
        .context("no control stream")??;

    let (stream_tx, mut stream_rx) = mpsc::channel::<OpenStreamReply>(32);
    let conn_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                Some(reply_tx) = stream_rx.recv() => {
                    let result = poll_fn(|cx| connection.poll_new_outbound(cx)).await;
                    let _ = reply_tx.send(result.map_err(|e| anyhow::anyhow!("{e}")));
                }
                inbound = poll_fn(|cx| connection.poll_next_inbound(cx)) => {
                    match inbound {
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            tlog::error(&format!("yamux error: {e}"));
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
    });

    let msg = protocol::read_msg(&mut control_stream).await?;
    let (requested_subdomain, nonce, provided_hmac) = match msg {
        ControlMsg::Auth {
            subdomain,
            nonce,
            hmac,
        } => (subdomain, nonce, hmac),
        _ => anyhow::bail!("expected Auth message"),
    };

    if let Some(secret) = expected_token {
        let expected_hmac = protocol::compute_hmac(secret, &nonce);
        if provided_hmac.as_deref() != Some(expected_hmac.as_str()) {
            let encoded = ControlMsg::Error {
                message: "invalid secret".into(),
            }
            .encode()?;
            control_stream.write_all(&encoded).await.ok();
            control_stream.close().await.ok();
            anyhow::bail!("invalid secret from {peer_ip}");
        }
    }

    let ip_active = ip_tracker
        .get(&peer_ip)
        .map(|s| s.active_tunnels)
        .unwrap_or(0);
    if ip_active >= MAX_TUNNELS_PER_IP {
        let encoded = ControlMsg::Error {
            message: format!("max {MAX_TUNNELS_PER_IP} tunnels per IP"),
        }
        .encode()?;
        control_stream.write_all(&encoded).await.ok();
        control_stream.close().await.ok();
        anyhow::bail!("{peer_ip} hit per-IP tunnel limit");
    }

    if registry.len() >= MAX_TUNNELS_TOTAL {
        let encoded = ControlMsg::Error {
            message: "server at capacity, try again later".into(),
        }
        .encode()?;
        control_stream.write_all(&encoded).await.ok();
        control_stream.close().await.ok();
        anyhow::bail!("global tunnel limit reached");
    }

    let subdomain = requested_subdomain
        .filter(|s| {
            !s.is_empty() && s.len() <= 63 && s.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
        .unwrap_or_else(|| {
            use rand::Rng;
            let mut rng = rand::rng();
            format!("{:08x}", rng.random::<u32>())
        });

    if registry.contains_key(&subdomain) {
        let encoded = ControlMsg::Error {
            message: format!("subdomain '{subdomain}' already in use"),
        }
        .encode()?;
        control_stream.write_all(&encoded).await.ok();
        control_stream.close().await.ok();
        anyhow::bail!("subdomain collision: {subdomain}");
    }

    let full_domain = format!("{subdomain}.{domain}");
    let ok = ControlMsg::AuthOk {
        subdomain: subdomain.clone(),
        url: full_domain.clone(),
    };
    control_stream.write_all(&ok.encode()?).await?;
    control_stream.flush().await?;

    registry.insert(subdomain.clone(), ClientHandle { stream_tx });
    ip_tracker
        .entry(peer_ip)
        .and_modify(|s| s.active_tunnels += 1);

    tlog::success(&format!(
        "tunnel live: {full_domain} (ip={peer_ip}, active={}/{MAX_TUNNELS_PER_IP})",
        ip_active + 1
    ));

    let mut buf = [0u8; 1024];
    loop {
        match control_stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }

    registry.remove(&subdomain);
    ip_tracker.entry(peer_ip).and_modify(|s| {
        if s.active_tunnels > 0 {
            s.active_tunnels -= 1;
        }
    });
    conn_task.abort();

    tlog::info(&format!("client disconnected, removed {full_domain}"));

    Ok(())
}

const INSTALL_SH: &str = include_str!("../install.sh");

async fn handle_http(
    mut socket: tokio::net::TcpStream,
    registry: Registry,
    domain: &str,
) -> Result<()> {
    let head = proxy::read_http_head(&mut socket).await?;
    let host = proxy::extract_host(&head).context("no Host header")?;

    if host == domain {
        let (_, path) = proxy::parse_request_line(&head);
        let response: Vec<u8> = if path == "/install.sh" {
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                INSTALL_SH.len(),
                INSTALL_SH
            )
            .into_bytes()
        } else {
            b"HTTP/1.1 301 Moved Permanently\r\nLocation: https://github.com/jbingen/tnnl\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
        };
        tokio::io::AsyncWriteExt::write_all(&mut socket, &response)
            .await
            .ok();
        return Ok(());
    }

    let subdomain = host
        .strip_suffix(&format!(".{domain}"))
        .context(format!("host '{host}' not a subdomain of {domain}"))?
        .to_string();

    let stream_tx = match registry.get(&subdomain) {
        Some(entry) => entry.stream_tx.clone(),
        None => {
            proxy::write_404(&mut socket).await.ok();
            return Ok(());
        }
    };

    let (reply_tx, reply_rx) = oneshot::channel();
    stream_tx
        .send(reply_tx)
        .await
        .map_err(|_| anyhow::anyhow!("client disconnected"))?;

    let tunnel_stream = reply_rx
        .await
        .map_err(|_| anyhow::anyhow!("client disconnected"))??;

    let mut tunnel_compat = tunnel_stream.compat();

    tokio::io::AsyncWriteExt::write_all(&mut tunnel_compat, &head).await?;

    tokio::io::copy_bidirectional(&mut socket, &mut tunnel_compat)
        .await
        .ok();

    Ok(())
}
