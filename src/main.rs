mod api;
mod capture;
mod config;
mod hotkey;
mod items;
mod logwatch;
mod market;
mod notify;
mod ocr;
mod overlay;
mod scanner;

use anyhow::{Context, Result, bail};
use config::Config;
use items::{Match, Vocab};
use market::{Market, Quote};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const GREEN: &str = "\x1b[1;32m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

const DEBOUNCE: Duration = Duration::from_secs(4);

pub enum Trigger {
    Log,
    Hotkey,
    Manual,
    Scan,
    Quit,
}

const LOG_DUP_WINDOW: Duration = Duration::from_secs(20);

struct Args {
    config: Option<PathBuf>,
    log: Option<PathBuf>,
    once: bool,
    scan_test: bool,
    dump: bool,
    save_shots: Option<PathBuf>,
}

struct Ctx<'a> {
    cfg: &'a Config,
    ocr: &'a ocr::Ocr,
    vocab: &'a Vocab,
    market: &'a Market,
    args: &'a Args,
    overlay: Option<&'a overlay::Handle>,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let cfg = load_config(args.config.as_deref())?;
    let log_path = cfg.resolve_log_path(args.log.as_deref());

    let t = Instant::now();
    let ocr = ocr::Ocr::new()?;
    eprintln!("OCR engine ready ({} ms)", t.elapsed().as_millis());

    let agent = api::new_agent();
    let (vocab, vocab_src) = Vocab::load(&agent, cfg.match_threshold)?;
    eprintln!("item vocabulary: {} names ({vocab_src})", vocab.len());
    let market = Market::new(agent)?;

    if args.scan_test {
        let (shot, source) = capture::capture(&cfg)?;
        let (fires, raw) = scanner::probe(&ocr, &shot, &cfg);
        println!(
            "{} | title read: \"{raw}\" | source: {source}",
            if fires { "WOULD TRIGGER" } else { "no trigger  " }
        );
        return Ok(());
    }

    if args.once {
        let ctx =
            Ctx { cfg: &cfg, ocr: &ocr, vocab: &vocab, market: &market, args: &args, overlay: None };
        return run_check(&ctx, "manual", &mut None).map(|_| ());
    }

    match cfg.capture {
        config::CaptureMode::Auto => eprintln!(
            "capture: auto (window \"{}\", else monitor {})",
            cfg.window_title, cfg.monitor
        ),
        config::CaptureMode::Window => eprintln!("capture: window \"{}\"", cfg.window_title),
        config::CaptureMode::Monitor => eprintln!("capture: monitor {} (always fresh)", cfg.monitor),
    }

    let overlay_tx = cfg.overlay.enabled.then(|| overlay::spawn(&cfg));

    let (tx, rx) = mpsc::channel();
    logwatch::spawn(log_path.clone(), tx.clone());
    if cfg.scan.enabled {
        scanner::spawn(cfg.clone(), tx.clone());
        eprintln!(
            "screen scan: every {}ms for the reward-screen title",
            cfg.scan.interval_ms
        );
    }
    hotkey::spawn_stdin(tx.clone());
    let keyboards = hotkey::spawn_f12(&tx);
    if keyboards == 0 {
        eprintln!(
            "{YELLOW}F12 hotkey unavailable: no readable /dev/input devices \
             (add your user to the `input` group and re-login).{RESET}"
        );
    } else {
        eprintln!("F12 hotkey armed on {keyboards} input device(s)");
    }
    eprintln!(
        "watching {} — crack a relic! (Enter = manual check, q + Enter = quit)",
        log_path.display()
    );

    let ctx = Ctx {
        cfg: &cfg,
        ocr: &ocr,
        vocab: &vocab,
        market: &market,
        args: &args,
        overlay: overlay_tx.as_ref(),
    };
    let mut last = Instant::now() - DEBOUNCE;
    let mut last_success: Option<Instant> = None;
    let mut last_result: Option<(Vec<String>, Instant)> = None;
    while let Ok(trigger) = rx.recv() {
        let label = match trigger {
            Trigger::Quit => break,
            Trigger::Log => "EE.log",
            Trigger::Hotkey => "F12",
            Trigger::Manual => "manual",
            Trigger::Scan => "scan",
        };
        if last.elapsed() < DEBOUNCE {
            continue;
        }
        if matches!(trigger, Trigger::Log)
            && last_success.is_some_and(|t| t.elapsed() < LOG_DUP_WINDOW)
        {
            continue;
        }
        last = Instant::now();
        match run_check(&ctx, label, &mut last_result) {
            Ok(true) => last_success = Some(Instant::now()),
            Ok(false) => {}
            Err(e) => eprintln!("{YELLOW}check failed: {e:#}{RESET}"),
        }
    }
    Ok(())
}

fn run_check(
    ctx: &Ctx,
    trigger: &str,
    last_result: &mut Option<(Vec<String>, Instant)>,
) -> Result<bool> {
    let Ctx { cfg, ocr, vocab, market, args, overlay: overlay_tx } = *ctx;
    let t0 = Instant::now();
    let (shot, source) = capture::capture(cfg)?;
    let band = capture::crop_band(&shot, &cfg.band, cfg.color_key);
    let t_capture = t0.elapsed();

    if args.dump {
        let dir = std::env::temp_dir().join("relic-check");
        capture::dump(&dir, &shot, &band).context("dumping calibration images")?;
        eprintln!("calibration images written to {}", dir.display());
    }

    let t1 = Instant::now();
    let raws = ocr.read_names(&band)?;
    let t_ocr = t1.elapsed();

    if std::env::var_os("RELIC_CHECK_DEBUG").is_some() {
        for (i, raw) in raws.iter().enumerate() {
            eprintln!("{DIM}column {}: \"{raw}\"{RESET}", i + 1);
        }
    }

    let mut rows: Vec<(String, Option<Match>)> = Vec::new();
    for raw in raws {
        let resolved = vocab.resolve(&raw);
        if resolved.is_empty() {
            rows.push((raw, None));
        } else {
            rows.extend(resolved.into_iter().map(|m| (raw.clone(), Some(m))));
        }
    }

    let matched = rows.iter().any(|(_, m)| m.is_some());
    if trigger == "scan" && !matched {
        return Ok(false);
    }

    if trigger == "EE.log" && !matched {
        eprintln!(
            "{DIM}EE.log trigger, but no reward names on screen — the log flush was \
             probably late and the screen is already gone.{RESET}"
        );
        return Ok(false);
    }

    let names: Vec<String> = rows
        .iter()
        .filter_map(|(_, m)| m.as_ref().map(|m| m.item.name.clone()))
        .collect();
    let automatic = matches!(trigger, "scan" | "EE.log");
    if matched && automatic && is_duplicate(last_result, &names, cfg.dedupe_secs) {
        return Ok(true);
    }
    if matched {
        *last_result = Some((names, Instant::now()));
    }

    let t2 = Instant::now();
    let quotes: Vec<Option<Quote>> = std::thread::scope(|s| {
        let handles: Vec<_> = rows
            .iter()
            .map(|(_, m)| {
                let item = m
                    .as_ref()
                    .and_then(|m| m.item.slug.clone().map(|s| (s, m.item.name.clone())));
                s.spawn(move || item.map(|(slug, name)| market.quote(&slug, &name)))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    let t_market = t2.elapsed();

    print_results(&rows, &quotes);
    eprintln!(
        "{DIM}trigger {trigger} | capture({source}) {}ms | ocr {}ms | market {}ms | total {}ms{RESET}",
        t_capture.as_millis(),
        t_ocr.as_millis(),
        t_market.as_millis(),
        t0.elapsed().as_millis()
    );

    if cfg.notify.enabled && matched {
        let (summary, body) = notify_text(&rows, &quotes);
        if let Err(e) = notify::send(&cfg.notify, &summary, &body) {
            eprintln!("{DIM}notification failed: {e:#}{RESET}");
        }
    }

    if let Some(tx) = overlay_tx
        && matched
    {
        let _ = tx.send(overlay::Msg::Show(overlay_cols(&rows, &quotes)));
    }

    if let Some(dir) = &args.save_shots {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating shot dir {}", dir.display()))?;
        let path = dir.join(format!("shot-{}-{trigger}.png", utc_stamp()));
        shot.save(&path).with_context(|| format!("saving {}", path.display()))?;
        eprintln!("{DIM}screenshot saved to {}{RESET}", path.display());
    }
    Ok(matched)
}

fn notify_text(
    rows: &[(String, Option<Match>)],
    quotes: &[Option<Quote>],
) -> (String, String) {
    let plat_of =
        |i: usize| quotes[i].as_ref().and_then(|q| q.price.map(|p| p.platinum));
    let best = (0..rows.len())
        .filter_map(|i| plat_of(i).map(|p| (p, i)))
        .max()
        .map(|(_, i)| i);
    let mut body = String::new();
    for (i, (_, m)) in rows.iter().enumerate() {
        let Some(m) = m else { continue };
        let plat = plat_of(i).map_or("—".into(), |p| format!("{p}p"));
        let ducats = quotes[i]
            .as_ref()
            .and_then(|q| q.ducats)
            .map_or("—".into(), |d| format!("{d}dc"));
        let vaulted = if quotes[i].as_ref().is_some_and(|q| q.vaulted == Some(true)) {
            " · vaulted"
        } else {
            ""
        };
        let line = format!("{} — {plat} · {ducats}{vaulted}", m.item.name);
        if best == Some(i) {
            body.push_str(&format!("<b>► {line}</b>\n"));
        } else {
            body.push_str(&format!("{line}\n"));
        }
    }
    let summary = match best.and_then(|i| rows[i].1.as_ref()) {
        Some(m) => format!("Pick: {} ({}p)", m.item.name, best.and_then(plat_of).unwrap_or(0)),
        None => "Relic rewards".into(),
    };
    (summary, body.trim_end().to_string())
}

fn overlay_cols(
    rows: &[(String, Option<Match>)],
    quotes: &[Option<Quote>],
) -> Vec<overlay::Col> {
    let plat_of =
        |i: usize| quotes[i].as_ref().and_then(|q| q.price.map(|p| p.platinum));
    let best = (0..rows.len())
        .filter_map(|i| plat_of(i).map(|p| (p, i)))
        .max()
        .map(|(_, i)| i);
    rows.iter()
        .enumerate()
        .filter_map(|(i, (_, m))| {
            let m = m.as_ref()?;
            let (value, mut sub) = match quotes[i].as_ref() {
                Some(q) => {
                    let ducats = q.ducats.map_or("—".into(), |d| format!("{d}dc"));
                    match q.price {
                        Some(p) => (
                            format!("{}p · {ducats}", p.platinum),
                            format!("{} {} sellers", p.sellers, p.pool),
                        ),
                        None => ("no sell orders".into(), String::new()),
                    }
                }
                None => ("untradeable".into(), String::new()),
            };
            if quotes[i].as_ref().is_some_and(|q| q.vaulted == Some(true)) {
                if !sub.is_empty() {
                    sub.push_str(" · ");
                }
                sub.push_str("VAULTED");
            }
            Some(overlay::Col { name: m.item.name.clone(), value, sub, best: best == Some(i) })
        })
        .collect()
}

fn is_duplicate(
    last: &Option<(Vec<String>, Instant)>,
    names: &[String],
    dedupe_secs: u64,
) -> bool {
    dedupe_secs > 0
        && last.as_ref().is_some_and(|(prev, at)| {
            prev == names && at.elapsed() < Duration::from_secs(dedupe_secs)
        })
}

fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let z = days as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{h:02}{m:02}{s:02}")
}

fn print_results(rows: &[(String, Option<Match>)], quotes: &[Option<Quote>]) {
    let plat_of = |i: usize| -> Option<u32> {
        quotes[i].as_ref().and_then(|q| q.price.map(|p| p.platinum))
    };
    let ducats_of = |i: usize| -> Option<u32> { quotes[i].as_ref().and_then(|q| q.ducats) };
    let best_plat = (0..rows.len())
        .filter_map(|i| plat_of(i).map(|p| (p, i)))
        .max()
        .map(|(_, i)| i);
    let best_ducats = (0..rows.len())
        .filter_map(|i| ducats_of(i).map(|d| (d, i)))
        .max()
        .map(|(_, i)| i);

    println!("\n──────────────── relic rewards ────────────────");
    for (i, (raw, m)) in rows.iter().enumerate() {
        let marker = if best_plat == Some(i) { "►" } else { " " };
        let (color, name, detail) = match m {
            Some(m) => {
                let mut detail = String::new();
                match quotes[i].as_ref() {
                    Some(q) => {
                        match q.price {
                            Some(p) => detail.push_str(&format!(
                                "{:>4}p  {:>4}  ({} {} sellers)",
                                p.platinum,
                                q.ducats.map_or("—".into(), |d| format!("{d}dc")),
                                p.sellers,
                                p.pool
                            )),
                            None => detail.push_str("no sell orders"),
                        }
                        if q.vaulted == Some(true) {
                            detail.push_str(&format!("  {YELLOW}VAULTED{RESET}"));
                        }
                        if let Some(e) = &q.error {
                            detail.push_str(&format!("  {YELLOW}[{e}]{RESET}"));
                        }
                    }
                    None => detail.push_str("   0p   untradeable"),
                }
                if m.score > 0.25 {
                    detail.push_str(&format!("  {DIM}(uncertain read: \"{raw}\"){RESET}"));
                }
                let color = if best_plat == Some(i) { GREEN } else { "" };
                (color, m.item.name.clone(), detail)
            }
            None if raw.trim().is_empty() => (DIM, "(empty)".into(), String::new()),
            None => (YELLOW, format!("unrecognized: \"{raw}\""), String::new()),
        };
        println!("{marker} {color}{name:<32}{RESET} {detail}");
    }

    if rows.iter().all(|(_, m)| m.is_none()) {
        println!(
            "{YELLOW}nothing recognized — is the reward screen up? If yes, recalibrate \
             the [band] region (run with --dump-crops and see README).{RESET}"
        );
    } else {
        if let Some(i) = best_plat {
            let name = rows[i].1.as_ref().map(|m| m.item.name.as_str()).unwrap_or("?");
            println!(
                "{GREEN}best plat:   {name} ({}p){RESET}",
                plat_of(i).unwrap_or(0)
            );
        }
        if let Some(i) = best_ducats
            && best_ducats != best_plat
        {
            let name = rows[i].1.as_ref().map(|m| m.item.name.as_str()).unwrap_or("?");
            println!("best ducats: {name} ({}dc)", ducats_of(i).unwrap_or(0));
        }
    }
}

fn load_config(cli_path: Option<&Path>) -> Result<Config> {
    if let Some(p) = cli_path {
        return Config::load(p);
    }
    let local = PathBuf::from("config.toml");
    if local.exists() {
        return Config::load(&local);
    }
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        })
        .join("relic-check/config.toml");
    if xdg.exists() {
        return Config::load(&xdg);
    }
    eprintln!("no config.toml found; using built-in defaults");
    Ok(Config::default())
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        config: None,
        log: None,
        once: false,
        scan_test: false,
        dump: false,
        save_shots: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => {
                args.config = Some(it.next().map(PathBuf::from).context("--config needs a path")?);
            }
            "--once" => args.once = true,
            "--scan-test" => args.scan_test = true,
            "--dump-crops" => args.dump = true,
            "--list-monitors" => {
                capture::list_monitors()?;
                std::process::exit(0);
            }
            "--save-shots" => {
                args.save_shots =
                    Some(it.next().map(PathBuf::from).context("--save-shots needs a directory")?);
            }
            "-h" | "--help" => {
                println!(
                    "relic-check — Warframe relic reward price checker\n\n\
                     usage: relic-check [OPTIONS] [EE_LOG_PATH]\n\n\
                     arguments:\n  \
                       EE_LOG_PATH       override the EE.log path (file or its directory)\n\n\
                     options:\n  \
                       --config <path>   config file (default: ./config.toml, then\n                    \
                                         $XDG_CONFIG_HOME/relic-check/config.toml)\n  \
                       --once            run a single check immediately and exit\n  \
                       --scan-test       report whether the screen scanner would fire on\n                    \
                                         the current screen, and the raw title read\n  \
                       --dump-crops      save screenshot + crops to /tmp/relic-check for\n                    \
                                         calibrating the crop regions\n  \
                       --list-monitors   print monitor indices for the config, then exit\n  \
                       --save-shots <dir>  archive every check's screenshot to <dir> as a\n                    \
                                         timestamped PNG (replayable via the\n                    \
                                         RELIC_CHECK_FAKE_SHOT env var)\n  \
                       -h, --help        show this help\n\n\
                     while running: Enter = manual check, F12 = global hotkey, q = quit"
                );
                std::process::exit(0);
            }
            other if other.starts_with('-') => bail!("unknown option: {other} (see --help)"),
            other => {
                if args.log.is_some() {
                    bail!("unexpected extra argument: {other}");
                }
                args.log = Some(PathBuf::from(other));
            }
        }
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn duplicate_same_rewards_within_window() {
        let last = Some((names(&["Lex Prime Barrel", "Forma Blueprint"]), Instant::now()));
        assert!(is_duplicate(&last, &names(&["Lex Prime Barrel", "Forma Blueprint"]), 30));
    }

    #[test]
    fn not_duplicate_when_order_or_items_differ() {
        let last = Some((names(&["Lex Prime Barrel", "Forma Blueprint"]), Instant::now()));
        assert!(!is_duplicate(&last, &names(&["Forma Blueprint", "Lex Prime Barrel"]), 30));
        assert!(!is_duplicate(&last, &names(&["Lex Prime Barrel"]), 30));
        assert!(!is_duplicate(&None, &names(&["Lex Prime Barrel"]), 30));
    }

    #[test]
    fn not_duplicate_when_window_expired_or_disabled() {
        let old = Instant::now() - Duration::from_secs(31);
        let last = Some((names(&["Forma Blueprint"]), old));
        assert!(!is_duplicate(&last, &names(&["Forma Blueprint"]), 30));
        let fresh = Some((names(&["Forma Blueprint"]), Instant::now()));
        assert!(!is_duplicate(&fresh, &names(&["Forma Blueprint"]), 0));
    }
}
