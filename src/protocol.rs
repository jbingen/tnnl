use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use futures::AsyncReadExt;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const MAX_CONTROL_MSG: usize = 4096;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlMsg {
    /// Client → Server: subdomain request + HMAC proof.
    /// Client generates `nonce`; `hmac` = HMAC-SHA256(secret, nonce) if secret is set.
    Auth {
        subdomain: Option<String>,
        nonce: String,
        hmac: Option<String>,
    },
    AuthOk {
        subdomain: String,
        url: String,
    },
    Error {
        message: String,
    },
}

impl ControlMsg {
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        let mut buf = serde_json::to_vec(self)?;
        buf.push(b'\n');
        Ok(buf)
    }

    fn decode(line: &[u8]) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(line)?)
    }
}

/// HMAC-SHA256(secret, nonce), base64-encoded.
pub fn compute_hmac(secret: &str, nonce: &str) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(nonce.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

/// Read a newline-delimited control message from a yamux stream.
pub async fn read_msg(stream: &mut yamux::Stream) -> anyhow::Result<ControlMsg> {
    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            anyhow::bail!("stream closed");
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > MAX_CONTROL_MSG {
            anyhow::bail!("control message too large");
        }
    }
    ControlMsg::decode(&buf)
}
