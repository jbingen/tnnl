use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRequest {
    pub id: u64,
    pub port: u16,
    pub method: String,
    pub path: String,
    /// Everything up to and including the trailing \r\n\r\n.
    pub raw_headers: String,
    /// Request body, base64-encoded to preserve binary payloads.
    pub body_b64: String,
}

fn store_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/tnnl-requests.jsonl")
}

/// Initialize the counter so IDs are globally unique across restarts.
pub fn init() {
    let max = read_all()
        .into_iter()
        .map(|r| r.id)
        .max()
        .unwrap_or(0);
    NEXT_ID.store(max + 1, Ordering::Relaxed);
}

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn store(req: StoredRequest) {
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(store_path())
    else {
        return;
    };
    if let Ok(line) = serde_json::to_string(&req) {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

pub fn find(id: u64) -> Option<StoredRequest> {
    read_all().into_iter().find(|r| r.id == id)
}

fn read_all() -> Vec<StoredRequest> {
    let Ok(content) = std::fs::read_to_string(store_path()) else {
        return vec![];
    };
    content
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}
