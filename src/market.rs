use crate::api;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use ureq::Agent;

const PRICE_TTL: Duration = Duration::from_secs(10 * 60);

const VAULT_TTL_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy)]
pub struct Price {
    pub platinum: u32,
    pub sellers: usize,

    pub pool: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct Quote {
    pub price: Option<Price>,
    pub ducats: Option<u32>,

    pub vaulted: Option<bool>,
    pub error: Option<String>,
}

pub struct Market {
    agent: Agent,
    prices: Mutex<HashMap<String, (Instant, Option<Price>)>>,

    ducats: Mutex<HashMap<String, u32>>,
    ducats_path: PathBuf,

    vaulted: Mutex<HashMap<String, (u64, Option<bool>)>>,
    vaulted_path: PathBuf,
}

#[derive(Deserialize)]
struct OrdersResponse {
    data: Vec<Order>,
}
#[derive(Deserialize)]
struct Order {
    #[serde(rename = "type")]
    kind: String,
    platinum: f64,
    #[serde(default)]
    visible: Option<bool>,
    user: Option<OrderUser>,
}
#[derive(Deserialize)]
struct OrderUser {
    status: Option<String>,
}
#[derive(Deserialize)]
struct ItemResponse {
    data: ItemInfo,
}
#[derive(Deserialize)]
struct ItemInfo {
    ducats: Option<u32>,
}
#[derive(Deserialize)]
struct VaultEntry {
    name: String,
    vaulted: Option<bool>,
}

impl Market {
    pub fn new(agent: Agent) -> Result<Market> {
        let cache = api::cache_dir()?;
        let ducats_path = cache.join("ducats.json");
        let ducats: HashMap<String, u32> = std::fs::read_to_string(&ducats_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let vaulted_path = cache.join("vaulted.json");
        let vaulted: HashMap<String, (u64, Option<bool>)> =
            std::fs::read_to_string(&vaulted_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
        Ok(Market {
            agent,
            prices: Mutex::new(HashMap::new()),
            ducats: Mutex::new(ducats),
            ducats_path,
            vaulted: Mutex::new(vaulted),
            vaulted_path,
        })
    }

    pub fn quote(&self, slug: &str, item_name: &str) -> Quote {
        let mut q = Quote::default();
        match self.price(slug) {
            Ok(p) => q.price = p,
            Err(e) => q.error = Some(format!("{e:#}")),
        }
        match self.ducat_value(slug) {
            Ok(d) => q.ducats = d,
            Err(e) => {
                if q.error.is_none() {
                    q.error = Some(format!("{e:#}"));
                }
            }
        }
        q.vaulted = self.vault_status(item_name);
        q
    }

    fn vault_status(&self, item_name: &str) -> Option<bool> {
        let parent = parent_prime(item_name)?;
        if let Some((at, v)) = self.vaulted.lock().unwrap().get(&parent)
            && now() < at + VAULT_TTL_SECS
        {
            return *v;
        }
        let url = format!(
            "https://api.warframestat.us/items/search/{}?only=name,vaulted",
            parent.replace(' ', "%20")
        );
        let found: Vec<VaultEntry> = match api::get_json(&self.agent, &url) {
            Ok(v) => v,
            Err(_) => return None,
        };
        let vaulted = found
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(&parent))
            .map(|e| e.vaulted.unwrap_or(false));
        let mut map = self.vaulted.lock().unwrap();
        map.insert(parent, (now(), vaulted));
        if let Ok(json) = serde_json::to_string(&*map) {
            let _ = std::fs::write(&self.vaulted_path, json);
        }
        vaulted
    }

    fn price(&self, slug: &str) -> Result<Option<Price>> {
        if let Some((at, price)) = self.prices.lock().unwrap().get(slug)
            && at.elapsed() < PRICE_TTL
        {
            return Ok(*price);
        }
        let url = format!("{}/orders/item/{}", api::BASE, slug);
        let resp: OrdersResponse =
            api::get_json(&self.agent, &url).context("fetching orders")?;
        let price = pick_price(&resp.data);
        self.prices
            .lock()
            .unwrap()
            .insert(slug.to_string(), (Instant::now(), price));
        Ok(price)
    }

    fn ducat_value(&self, slug: &str) -> Result<Option<u32>> {
        if let Some(d) = self.ducats.lock().unwrap().get(slug) {
            return Ok(Some(*d));
        }
        let url = format!("{}/items/{}", api::BASE, slug);
        let resp: ItemResponse =
            api::get_json(&self.agent, &url).context("fetching item info")?;
        if let Some(d) = resp.data.ducats {
            let mut map = self.ducats.lock().unwrap();
            map.insert(slug.to_string(), d);
            if let Ok(json) = serde_json::to_string(&*map) {
                let _ = std::fs::write(&self.ducats_path, json);
            }
        }
        Ok(resp.data.ducats)
    }
}

fn parent_prime(name: &str) -> Option<String> {
    let words: Vec<&str> = name.split_whitespace().collect();
    let idx = words.iter().position(|w| *w == "Prime")?;
    Some(words[..=idx].join(" "))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn pick_price(orders: &[Order]) -> Option<Price> {
    let sells: Vec<&Order> = orders
        .iter()
        .filter(|o| o.kind == "sell" && o.visible.unwrap_or(true))
        .collect();
    for pool in ["ingame", "online"] {
        let in_pool: Vec<&&Order> = sells
            .iter()
            .filter(|o| {
                o.user
                    .as_ref()
                    .and_then(|u| u.status.as_deref())
                    .is_some_and(|s| s == pool)
            })
            .collect();
        if let Some(min) = in_pool
            .iter()
            .map(|o| o.platinum.round() as u32)
            .min()
        {
            return Some(Price { platinum: min, sellers: in_pool.len(), pool });
        }
    }
    sells
        .iter()
        .map(|o| o.platinum.round() as u32)
        .min()
        .map(|min| Price { platinum: min, sellers: sells.len(), pool: "any" })
}
