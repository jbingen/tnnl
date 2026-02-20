use anyhow::{bail, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};


const MAX_HEADER_SIZE: usize = 8192;

const HTTP_502: &[u8] = b"HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\nContent-Length: 39\r\nConnection: close\r\n\r\nFailed to connect to upstream service.\n";

const HTTP_404: &[u8] = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 26\r\nConnection: close\r\n\r\nNo tunnel for this domain\n";

const HTTP_401: &[u8] = b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"tnnl\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

pub fn extract_host(buf: &[u8]) -> Option<String> {
    let header_str = std::str::from_utf8(buf).ok()?;
    for line in header_str.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        if line.len() > 5 && line[..5].eq_ignore_ascii_case("host:") {
            let host = line[5..].trim();
            return Some(host.split(':').next().unwrap_or(host).to_lowercase());
        }
    }
    None
}

/// Read from a TCP stream until we have the full HTTP header block.
/// Returns the buffered bytes (headers + any body bytes already received).
pub async fn read_http_head<R: AsyncRead + Unpin>(stream: &mut R) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            bail!("connection closed before headers complete");
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > MAX_HEADER_SIZE {
            bail!("headers exceed max size");
        }
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(buf);
        }
    }
}

pub async fn write_502<W: AsyncWrite + Unpin>(stream: &mut W) -> Result<()> {
    stream.write_all(HTTP_502).await?;
    stream.flush().await?;
    Ok(())
}

pub async fn write_401<W: AsyncWrite + Unpin>(stream: &mut W) -> Result<()> {
    stream.write_all(HTTP_401).await?;
    stream.flush().await?;
    Ok(())
}

/// Returns the value of the Authorization header, or None.
pub fn extract_authorization(buf: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(buf).ok()?;
    for line in text.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        if line.len() > 14 && line[..14].eq_ignore_ascii_case("authorization:") {
            return Some(line[14..].trim().to_string());
        }
    }
    None
}

pub async fn write_404<W: AsyncWrite + Unpin>(stream: &mut W) -> Result<()> {
    stream.write_all(HTTP_404).await?;
    stream.flush().await?;
    Ok(())
}

pub fn is_chunked(headers: &[u8]) -> bool {
    let text = std::str::from_utf8(headers).unwrap_or("");
    for line in text.split("\r\n") {
        if let Some(colon) = line.find(':') {
            if line[..colon].eq_ignore_ascii_case("transfer-encoding") {
                return line[colon + 1..].trim().eq_ignore_ascii_case("chunked");
            }
        }
    }
    false
}

/// Index of the first byte after the header block (i.e. after \r\n\r\n).
pub fn headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

pub fn parse_content_length(buf: &[u8]) -> usize {
    let text = std::str::from_utf8(buf).unwrap_or("");
    for line in text.split("\r\n") {
        if let Some(colon) = line.find(':') {
            if line[..colon].eq_ignore_ascii_case("content-length") {
                return line[colon + 1..].trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

/// Extract the status code from the first line of an HTTP response.
pub fn parse_response_status(buf: &[u8]) -> u16 {
    // "HTTP/1.1 200 OK\r\n..."
    std::str::from_utf8(buf)
        .unwrap_or("")
        .split_ascii_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(502)
}

/// Try to extract method and path from the first line of an HTTP request.
pub fn parse_request_line(buf: &[u8]) -> (String, String) {
    let s = match std::str::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => return ("?".into(), "?".into()),
    };
    let first_line = s.split("\r\n").next().unwrap_or("");
    let parts: Vec<&str> = first_line.splitn(3, ' ').collect();
    if parts.len() >= 2 {
        (parts[0].to_string(), parts[1].to_string())
    } else {
        ("?".into(), "?".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_basic() {
        let req = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        assert_eq!(extract_host(req), Some("example.com".into()));
    }

    #[test]
    fn extract_host_strips_port() {
        let req = b"GET / HTTP/1.1\r\nHost: example.com:8080\r\n\r\n";
        assert_eq!(extract_host(req), Some("example.com".into()));
    }

    #[test]
    fn extract_host_case_insensitive() {
        let req = b"GET / HTTP/1.1\r\nHOST: Example.COM\r\n\r\n";
        assert_eq!(extract_host(req), Some("example.com".into()));
    }

    #[test]
    fn extract_host_missing() {
        let req = b"GET / HTTP/1.1\r\nContent-Type: text/plain\r\n\r\n";
        assert_eq!(extract_host(req), None);
    }

    #[test]
    fn parse_request_line_get() {
        let req = b"GET /api/users HTTP/1.1\r\nHost: x\r\n\r\n";
        assert_eq!(parse_request_line(req), ("GET".into(), "/api/users".into()));
    }

    #[test]
    fn parse_request_line_post_with_query() {
        let req = b"POST /webhook?secret=abc HTTP/1.1\r\n\r\n";
        assert_eq!(
            parse_request_line(req),
            ("POST".into(), "/webhook?secret=abc".into())
        );
    }

    #[test]
    fn parse_request_line_garbage() {
        assert_eq!(parse_request_line(b"nothttp"), ("?".into(), "?".into()));
    }

    #[test]
    fn parse_response_status_200() {
        assert_eq!(parse_response_status(b"HTTP/1.1 200 OK\r\n"), 200);
    }

    #[test]
    fn parse_response_status_404() {
        assert_eq!(parse_response_status(b"HTTP/1.1 404 Not Found\r\n"), 404);
    }

    #[test]
    fn parse_response_status_empty_returns_502() {
        assert_eq!(parse_response_status(b""), 502);
    }

    #[test]
    fn parse_content_length_present() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: 42\r\n\r\n";
        assert_eq!(parse_content_length(headers), 42);
    }

    #[test]
    fn parse_content_length_missing() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n";
        assert_eq!(parse_content_length(headers), 0);
    }

    #[test]
    fn parse_content_length_case_insensitive() {
        let headers = b"HTTP/1.1 200 OK\r\ncontent-length: 99\r\n\r\n";
        assert_eq!(parse_content_length(headers), 99);
    }

    #[test]
    fn is_chunked_true() {
        let headers = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(is_chunked(headers));
    }

    #[test]
    fn is_chunked_false_when_missing() {
        let headers = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n";
        assert!(!is_chunked(headers));
    }

    #[test]
    fn is_chunked_case_insensitive() {
        let headers = b"HTTP/1.1 200 OK\r\ntransfer-encoding: CHUNKED\r\n\r\n";
        assert!(is_chunked(headers));
    }

    #[test]
    fn headers_end_found() {
        let buf = b"GET / HTTP/1.1\r\nHost: x\r\n\r\nbody";
        let end = headers_end(buf).unwrap();
        assert_eq!(&buf[end..], b"body");
    }

    #[test]
    fn headers_end_not_found() {
        assert_eq!(headers_end(b"GET / HTTP/1.1\r\nHost: x"), None);
    }
}
