use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub server: Option<String>,
    pub token: Option<String>,
    pub control_port: Option<u16>,
    pub subdomain: Option<String>,
    pub inspect: Option<bool>,
    pub auth: Option<String>,
}

pub fn load() -> Config {
    let path = config_path();
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    toml::from_str(&contents).unwrap_or_default()
}

fn config_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tnnl.toml")
}
