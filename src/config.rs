use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMode {
    #[default]
    Auto,

    Window,

    Monitor,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub log_path: Option<PathBuf>,

    #[serde(default = "default_window_title")]
    pub window_title: String,

    #[serde(default)]
    pub capture: CaptureMode,

    #[serde(default)]
    pub monitor: usize,

    #[serde(default = "default_match_threshold")]
    pub match_threshold: f64,

    #[serde(default = "default_color_key")]
    pub color_key: bool,

    #[serde(default = "default_dedupe_secs")]
    pub dedupe_secs: u64,

    #[serde(default = "default_band")]
    pub band: Region,

    #[serde(default)]
    pub scan: ScanConfig,

    #[serde(default)]
    pub notify: NotifyConfig,

    #[serde(default)]
    pub overlay: OverlayConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_overlay_timeout_ms")]
    pub timeout_ms: u64,

    #[serde(default)]
    pub y: Option<f32>,

    #[serde(default = "default_overlay_height")]
    pub height: f32,

    #[serde(default)]
    pub font_path: Option<PathBuf>,
}

fn default_overlay_timeout_ms() -> u64 {
    15_000
}

fn default_overlay_height() -> f32 {
    0.075
}

impl Default for OverlayConfig {
    fn default() -> Self {
        OverlayConfig {
            enabled: false,
            timeout_ms: default_overlay_timeout_ms(),
            y: None,
            height: default_overlay_height(),
            font_path: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_notify_timeout_ms")]
    pub timeout_ms: i32,

    #[serde(default = "default_notify_urgency")]
    pub urgency: String,
}

fn default_notify_timeout_ms() -> i32 {
    15_000
}

fn default_notify_urgency() -> String {
    "critical".into()
}

impl Default for NotifyConfig {
    fn default() -> Self {
        NotifyConfig {
            enabled: false,
            timeout_ms: default_notify_timeout_ms(),
            urgency: default_notify_urgency(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_scan_interval_ms")]
    pub interval_ms: u64,

    #[serde(flatten, default = "default_scan_region")]
    pub region: Region,
}

fn default_scan_interval_ms() -> u64 {
    1000
}

fn default_scan_region() -> Region {
    Region { x: 0.13, y: 0.028, w: 0.33, h: 0.06 }
}

impl Default for ScanConfig {
    fn default() -> Self {
        ScanConfig {
            enabled: false,
            interval_ms: default_scan_interval_ms(),
            region: default_scan_region(),
        }
    }
}

fn default_window_title() -> String {
    "Warframe".into()
}

fn default_match_threshold() -> f64 {
    0.45
}

fn default_dedupe_secs() -> u64 {
    30
}

fn default_color_key() -> bool {
    false
}

fn default_band() -> Region {
    Region { x: 0.24, y: 0.372, w: 0.52, h: 0.062 }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            log_path: None,
            window_title: default_window_title(),
            capture: CaptureMode::Auto,
            monitor: 0,
            match_threshold: default_match_threshold(),
            color_key: default_color_key(),
            dedupe_secs: default_dedupe_secs(),
            band: default_band(),
            scan: ScanConfig::default(),
            notify: NotifyConfig::default(),
            overlay: OverlayConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parsing config {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        for (what, r) in [("[band]", &self.band), ("[scan]", &self.scan.region)] {
            let ok = (0.0..=1.0).contains(&r.x)
                && (0.0..=1.0).contains(&r.y)
                && r.w > 0.0
                && r.h > 0.0
                && r.x + r.w <= 1.0 + f32::EPSILON
                && r.y + r.h <= 1.0 + f32::EPSILON;
            if !ok {
                bail!(
                    "{what} is out of bounds (values are fractions of the screen, 0.0-1.0): {r:?}"
                );
            }
        }
        Ok(())
    }

    pub fn resolve_log_path(&self, cli_override: Option<&Path>) -> PathBuf {
        let base = cli_override
            .map(Path::to_path_buf)
            .or_else(|| self.log_path.clone())
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME").unwrap_or_default();
                PathBuf::from(home).join(
                    ".local/share/Steam/steamapps/compatdata/230410/pfx/drive_c/users/steamuser/AppData/Local/Warframe/EE.log",
                )
            });
        if base.is_dir() { base.join("EE.log") } else { base }
    }
}
