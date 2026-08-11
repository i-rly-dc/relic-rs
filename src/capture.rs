use crate::config::{CaptureMode, Config, Region};
use anyhow::{Context, Result, bail};
use image::imageops::{self, FilterType};
use image::{DynamicImage, RgbImage, RgbaImage};
use xcap::{Monitor, Window};

pub fn capture(cfg: &Config) -> Result<(RgbaImage, &'static str)> {
    if let Some(path) = std::env::var_os("RELIC_CHECK_FAKE_SHOT") {
        let img = image::open(&path)
            .with_context(|| format!("loading RELIC_CHECK_FAKE_SHOT {}", path.display()))?
            .to_rgba8();
        return Ok((img, "file"));
    }
    if cfg.capture != CaptureMode::Monitor
        && let Some(img) = capture_window(&cfg.window_title)
    {
        return Ok((img, "window"));
    }
    if cfg.capture == CaptureMode::Window {
        bail!(
            "no capturable window with \"{}\" in its title (capture = \"window\")",
            cfg.window_title
        );
    }
    let monitors = Monitor::all().context("listing monitors")?;
    if monitors.is_empty() {
        bail!("no monitors found");
    }
    let monitor = monitors
        .get(cfg.monitor)
        .or_else(|| {
            monitors
                .iter()
                .find(|m| m.is_primary().unwrap_or(false))
        })
        .unwrap_or(&monitors[0]);
    let img = monitor
        .capture_image()
        .context("capturing monitor (on Wayland this needs xdg-desktop-portal)")?;
    Ok((img, "monitor"))
}

pub fn list_monitors() -> Result<()> {
    let monitors = Monitor::all().context("listing monitors")?;
    for (i, m) in monitors.iter().enumerate() {
        println!(
            "{i}: {} {}x{} at ({},{}){}",
            m.name().unwrap_or_else(|_| "?".into()),
            m.width().unwrap_or(0),
            m.height().unwrap_or(0),
            m.x().unwrap_or(0),
            m.y().unwrap_or(0),
            if m.is_primary().unwrap_or(false) { " [primary]" } else { "" },
        );
    }
    Ok(())
}

pub(crate) fn capture_window(title: &str) -> Option<RgbaImage> {
    let needle = title.to_lowercase();
    let windows = Window::all().ok()?;
    let win = windows.iter().find(|w| {
        if w.is_minimized().unwrap_or(false) {
            return false;
        }
        w.title().unwrap_or_default().to_lowercase().contains(&needle)
            || w.app_name().unwrap_or_default().to_lowercase().contains(&needle)
    })?;
    win.capture_image().ok()
}

pub fn crop_band(shot: &RgbaImage, r: &Region, color_key: bool) -> RgbImage {
    let (sw, sh) = shot.dimensions();
    let x = ((r.x * sw as f32) as u32).min(sw.saturating_sub(1));
    let y = ((r.y * sh as f32) as u32).min(sh.saturating_sub(1));
    let w = ((r.w * sw as f32) as u32).clamp(1, sw - x);
    let h = ((r.h * sh as f32) as u32).clamp(1, sh - y);
    let crop = imageops::crop_imm(shot, x, y, w, h).to_image();
    let mut rgb = DynamicImage::ImageRgba8(crop).to_rgb8();
    if color_key {
        key_text(&mut rgb);
    }
    let factor = (128 / h.max(1)).clamp(1, 3);
    if factor > 1 {
        rgb = imageops::resize(&rgb, w * factor, h * factor, FilterType::CatmullRom);
    }
    rgb
}

fn key_text(img: &mut image::RgbImage) {
    for p in img.pixels_mut() {
        let [r, g, b] = p.0;
        let (r_i, g_i, b_i) = (r as i32, g as i32, b as i32);
        let is_text = r_i > 190 && (70..=190).contains(&g_i) && b_i < 110 && r_i - b_i > 120;
        let v = if is_text { r } else { 0 };
        p.0 = [v, v, v];
    }
}

pub fn dump(dir: &std::path::Path, shot: &RgbaImage, band: &RgbImage) -> Result<()> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    shot.save(dir.join("screenshot.png"))?;
    band.save(dir.join("band.png"))?;
    Ok(())
}
