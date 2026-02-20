use std::time::Duration;

const CURRENT: &str = env!("CARGO_PKG_VERSION");
const API_URL: &str = "https://api.github.com/repos/jbingen/tnnl/releases/latest";

pub fn check_in_background() {
    tokio::spawn(async {
        if let Some(latest) = fetch_latest().await {
            let latest = latest.trim_start_matches('v');
            if latest != CURRENT {
                crate::log::update_available(latest);
            }
        }
    });
}

async fn fetch_latest() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .user_agent(format!("tnnl/{CURRENT}"))
        .build()
        .ok()?;

    let resp: serde_json::Value = client.get(API_URL).send().await.ok()?.json().await.ok()?;

    resp["tag_name"].as_str().map(|s| s.to_owned())
}
