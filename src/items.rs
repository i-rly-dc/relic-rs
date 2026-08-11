use crate::api;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use ureq::Agent;

const CACHE_TTL_SECS: u64 = 24 * 60 * 60;

const UNTRADEABLE: &[&str] = &[
    "Forma Blueprint",
    "Riven Sliver",
    "Ayatan Amber Star",
    "Exilus Weapon Adapter Blueprint",
    "1,200 Kuva",
    "1,400 Kuva",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VocabItem {
    pub name: String,

    pub slug: Option<String>,
    #[serde(skip)]
    norm: String,
}

#[derive(Debug, Clone)]
pub struct Match {
    pub item: VocabItem,

    pub score: f64,
}

pub struct Vocab {
    items: Vec<VocabItem>,
    threshold: f64,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    fetched_at: u64,
    items: Vec<VocabItem>,
}

#[derive(Deserialize)]
struct ItemsResponse {
    data: Vec<RawItem>,
}
#[derive(Deserialize)]
struct RawItem {
    slug: String,
    i18n: RawI18n,
}
#[derive(Deserialize)]
struct RawI18n {
    en: Option<RawName>,
}
#[derive(Deserialize)]
struct RawName {
    name: String,
}

impl Vocab {
    pub fn load(agent: &Agent, threshold: f64) -> Result<(Vocab, &'static str)> {
        let cache_path = api::cache_dir()?.join("items.json");
        let cached: Option<CacheFile> = std::fs::read_to_string(&cache_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());

        if let Some(c) = &cached
            && now() < c.fetched_at + CACHE_TTL_SECS
        {
            return Ok((Vocab::new(c.items.clone(), threshold), "cached"));
        }

        match fetch_items(agent) {
            Ok(items) => {
                let file = CacheFile { fetched_at: now(), items };
                if let Ok(json) = serde_json::to_string(&file) {
                    let _ = std::fs::write(&cache_path, json);
                }
                Ok((Vocab::new(file.items, threshold), "fetched"))
            }
            Err(e) => match cached {
                Some(c) => {
                    eprintln!("warning: item list refresh failed ({e:#}); using stale cache");
                    Ok((Vocab::new(c.items, threshold), "stale cache"))
                }
                None => Err(e.context("fetching item list (no cache available yet)")),
            },
        }
    }

    fn new(mut items: Vec<VocabItem>, threshold: f64) -> Vocab {
        for it in &mut items {
            it.norm = normalize(&it.name);
        }

        for name in UNTRADEABLE {
            if !items.iter().any(|i| i.name == *name) {
                items.push(VocabItem {
                    name: name.to_string(),
                    slug: None,
                    norm: normalize(name),
                });
            }
        }
        Vocab { items, threshold }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn best_match(&self, raw: &str) -> Option<Match> {
        let norm = normalize(raw);
        if norm.is_empty() {
            return None;
        }
        let (item, dist) = self
            .items
            .iter()
            .map(|it| (it, strsim::levenshtein(&norm, &it.norm)))
            .min_by_key(|&(_, d)| d)?;
        let score = dist as f64 / norm.len().max(item.norm.len()) as f64;
        (score <= self.threshold).then(|| Match { item: item.clone(), score })
    }

    pub fn resolve(&self, raw: &str) -> Vec<Match> {
        let best = self.best_match(raw);
        let suspicious = match &best {
            Some(m) => {
                m.score > 0.25 && normalize(raw).len() > (m.item.norm.len() * 7) / 5
            }
            None => true,
        };
        if !suspicious {
            return best.into_iter().collect();
        }
        let split = self.segment(raw);

        match (split.len(), best) {
            (0, b) => b.into_iter().collect(),
            (1, Some(b)) if split[0].score >= b.score => vec![b],
            _ => split,
        }
    }

    pub fn segment(&self, raw: &str) -> Vec<Match> {
        let s = normalize(raw);
        let n = s.len();
        if n == 0 {
            return Vec::new();
        }

        const SKIP_COST: f64 = 0.9;

        let mut dp: Vec<Option<(f64, usize, Option<Match>)>> = vec![None; n + 1];
        dp[0] = Some((0.0, 0, None));
        for i in 0..n {
            let Some((cost_i, _, _)) = dp[i].clone() else { continue };
            if s.as_bytes()[i] == b' ' {
                relax(&mut dp, i + 1, cost_i, i, None);
                continue;
            }

            let word_end = s[i..].find(' ').map_or(n, |p| i + p);
            relax(
                &mut dp,
                word_end,
                cost_i + SKIP_COST * (word_end - i) as f64,
                i,
                None,
            );

            for it in &self.items {
                let end = (i + it.norm.len()).min(n);
                let span = &s[i..end];
                let dist = strsim::levenshtein(span, &it.norm);
                let score = dist as f64 / it.norm.len().max(span.len()) as f64;
                if score <= self.threshold {
                    let m = Match { item: it.clone(), score };
                    relax(&mut dp, end, cost_i + score, i, Some(m));
                }
            }
        }

        let mut out = Vec::new();
        let mut at = n;
        while at > 0 {
            let Some((_, prev, m)) = dp[at].clone() else { break };
            if let Some(m) = m {
                out.push(m);
            }
            at = prev;
        }
        out.reverse();
        out
    }
}

fn relax(
    dp: &mut [Option<(f64, usize, Option<Match>)>],
    to: usize,
    cost: f64,
    from: usize,
    m: Option<Match>,
) {
    if dp[to].as_ref().is_none_or(|(best, _, _)| cost < *best) {
        dp[to] = Some((cost, from, m));
    }
}

fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_uppercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

fn fetch_items(agent: &Agent) -> Result<Vec<VocabItem>> {
    let resp: ItemsResponse = api::get_json(agent, &format!("{}/items", api::BASE))
        .context("downloading warframe.market item list")?;
    let items: Vec<VocabItem> = resp
        .data
        .into_iter()
        .filter_map(|raw| {
            let name = raw.i18n.en?.name;

            let is_part = name.split_whitespace().any(|w| w == "Prime")
                && !name.ends_with(" Set");
            is_part.then_some(VocabItem { name, slug: Some(raw.slug), norm: String::new() })
        })
        .collect();
    Ok(items)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vocab {
        let names = [
            "Revenant Prime Blueprint",
            "Revenant Prime Systems Blueprint",
            "Masseter Prime Handle",
            "Masseter Prime Blade",
            "Braton Prime Stock",
            "Trumna Prime Blueprint",
            "Forma Blueprint",
        ];
        Vocab::new(
            names
                .iter()
                .map(|n| VocabItem { name: n.to_string(), slug: None, norm: String::new() })
                .collect(),
            0.45,
        )
    }

    fn seg_names(raw: &str) -> Vec<String> {
        vocab().segment(raw).into_iter().map(|m| m.item.name).collect()
    }

    #[test]
    fn segments_merged_cluster() {
        assert_eq!(
            seg_names("Revenant Prime Blueprint Masseter Prime Handle WN"),
            ["Revenant Prime Blueprint", "Masseter Prime Handle"]
        );
    }

    #[test]
    fn segments_fused_words() {
        assert_eq!(
            seg_names("Revenant Prime BlueprintMasseter Prime Hande WW K"),
            ["Revenant Prime Blueprint", "Masseter Prime Handle"]
        );
    }

    #[test]
    fn segment_rejects_garbage() {
        assert!(seg_names("20, 2551)) ie FooAE").is_empty());
        assert!(seg_names("WN").is_empty());
    }

    #[test]
    fn best_match_snaps_noisy_read() {
        let m = vocab().best_match("8raton Prlme Stocx").unwrap();
        assert_eq!(m.item.name, "Braton Prime Stock");
    }

    #[test]
    fn recognizes_untradeable_kuva_rewards() {
        let v = vocab();
        assert_eq!(v.best_match("Riven Sliver").unwrap().item.name, "Riven Sliver");
        assert_eq!(v.best_match("1200 KUVA").unwrap().item.name, "1,200 Kuva");
        assert_eq!(v.best_match("Forma Blueprint").unwrap().item.slug, None);
    }
}
