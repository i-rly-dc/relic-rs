use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::time::Duration;
use ureq::Agent;

pub const BASE: &str = "https://api.warframe.market/v2";

pub fn new_agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(concat!(
            "relic-check/",
            env!("CARGO_PKG_VERSION"),
            " (Warframe relic reward price checker)"
        ))
        .build()
        .new_agent()
}

pub fn get_json<T: DeserializeOwned>(agent: &Agent, url: &str) -> Result<T> {
    let body = agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading response of {url}"))?;
    serde_json::from_str(&body).with_context(|| format!("unexpected JSON from {url}"))
}

pub fn cache_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".cache")
        });
    let dir = base.join("relic-check");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating cache dir {}", dir.display()))?;
    Ok(dir)
}
