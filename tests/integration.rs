use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const SECRET: &str = "test-secret";
const DOMAIN: &str = "tunnel.test";

async fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn start_backend(port: u16) {
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut conn, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = conn.read(&mut buf).await;
                let body = b"{\"ok\":true}";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = conn.write_all(resp.as_bytes()).await;
                let _ = conn.write_all(body).await;
            });
        }
    });
}

#[tokio::test]
async fn tunnel_proxies_request_end_to_end() {
    let control_port = free_port().await;
    let http_port = free_port().await;
    let backend_port = free_port().await;

    tokio::spawn(tnnl::server::run(
        control_port,
        http_port,
        DOMAIN,
        Some(SECRET),
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;

    start_backend(backend_port).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    tokio::spawn(tnnl::client::run(tnnl::client::TunnelOpts {
        local_port: backend_port,
        local_host: "127.0.0.1",
        server_addr: "127.0.0.1",
        server_port: control_port,
        token: SECRET,
        subdomain: Some("testapp"),
        auth: None,
        inspect: false,
    }));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut conn = TcpStream::connect(format!("127.0.0.1:{http_port}"))
        .await
        .expect("connect to tnnl http port");

    let req = format!("GET /ping HTTP/1.1\r\nHost: testapp.{DOMAIN}\r\nConnection: close\r\n\r\n");
    conn.write_all(req.as_bytes()).await.unwrap();

    let mut resp = Vec::new();
    conn.read_to_end(&mut resp).await.unwrap();
    let resp_str = String::from_utf8_lossy(&resp);

    assert!(resp_str.contains("200"), "expected 200, got: {resp_str}");
    assert!(
        resp_str.contains("{\"ok\":true}"),
        "expected JSON body, got: {resp_str}"
    );
}

#[tokio::test]
async fn tunnel_rejects_wrong_secret() {
    let control_port = free_port().await;
    let http_port = free_port().await;
    let backend_port = free_port().await;

    tokio::spawn(tnnl::server::run(
        control_port,
        http_port,
        DOMAIN,
        Some(SECRET),
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;

    start_backend(backend_port).await;

    tokio::spawn(tnnl::client::run(tnnl::client::TunnelOpts {
        local_port: backend_port,
        local_host: "127.0.0.1",
        server_addr: "127.0.0.1",
        server_port: control_port,
        token: "wrong-secret",
        subdomain: Some("badapp"),
        auth: None,
        inspect: false,
    }));
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The subdomain should not be registered so we get 404.
    let mut conn = TcpStream::connect(format!("127.0.0.1:{http_port}"))
        .await
        .unwrap();
    let req = format!("GET / HTTP/1.1\r\nHost: badapp.{DOMAIN}\r\nConnection: close\r\n\r\n");
    conn.write_all(req.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    conn.read_to_end(&mut resp).await.unwrap();
    assert!(String::from_utf8_lossy(&resp).contains("404"));
}

#[tokio::test]
async fn tunnel_works_with_no_secret() {
    let control_port = free_port().await;
    let http_port = free_port().await;
    let backend_port = free_port().await;

    tokio::spawn(tnnl::server::run(control_port, http_port, DOMAIN, None));
    tokio::time::sleep(Duration::from_millis(50)).await;

    start_backend(backend_port).await;

    tokio::spawn(tnnl::client::run(tnnl::client::TunnelOpts {
        local_port: backend_port,
        local_host: "127.0.0.1",
        server_addr: "127.0.0.1",
        server_port: control_port,
        token: "",
        subdomain: Some("openapp"),
        auth: None,
        inspect: false,
    }));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut conn = TcpStream::connect(format!("127.0.0.1:{http_port}"))
        .await
        .unwrap();
    let req = format!("GET / HTTP/1.1\r\nHost: openapp.{DOMAIN}\r\nConnection: close\r\n\r\n");
    conn.write_all(req.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    conn.read_to_end(&mut resp).await.unwrap();
    assert!(String::from_utf8_lossy(&resp).contains("200"));
}

#[tokio::test]
async fn tunnel_returns_404_for_unknown_subdomain() {
    let control_port = free_port().await;
    let http_port = free_port().await;

    tokio::spawn(tnnl::server::run(control_port, http_port, DOMAIN, None));
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut conn = TcpStream::connect(format!("127.0.0.1:{http_port}"))
        .await
        .unwrap();
    let req = format!("GET / HTTP/1.1\r\nHost: nobody.{DOMAIN}\r\nConnection: close\r\n\r\n");
    conn.write_all(req.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    conn.read_to_end(&mut resp).await.unwrap();
    assert!(String::from_utf8_lossy(&resp).contains("404"));
}

#[tokio::test]
async fn tunnel_returns_502_when_backend_down() {
    let control_port = free_port().await;
    let http_port = free_port().await;
    let dead_port = free_port().await;

    tokio::spawn(tnnl::server::run(
        control_port,
        http_port,
        DOMAIN,
        Some(SECRET),
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::spawn(tnnl::client::run(tnnl::client::TunnelOpts {
        local_port: dead_port,
        local_host: "127.0.0.1",
        server_addr: "127.0.0.1",
        server_port: control_port,
        token: SECRET,
        subdomain: Some("deadapp"),
        auth: None,
        inspect: false,
    }));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut conn = TcpStream::connect(format!("127.0.0.1:{http_port}"))
        .await
        .unwrap();
    let req = format!("GET / HTTP/1.1\r\nHost: deadapp.{DOMAIN}\r\nConnection: close\r\n\r\n");
    conn.write_all(req.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    conn.read_to_end(&mut resp).await.unwrap();
    assert!(String::from_utf8_lossy(&resp).contains("502"));
}
