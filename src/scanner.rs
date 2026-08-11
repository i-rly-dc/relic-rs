use crate::config::Config;
use crate::{DIM, RESET, Trigger};
use crate::{capture, ocr::Ocr};
use image::imageops::{self, FilterType};
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

pub fn spawn(cfg: Config, tx: Sender<Trigger>) {
    thread::spawn(move || {
        let ocr = match Ocr::new() {
            Ok(o) => o,
            Err(e) => {
                eprintln!("scanner: OCR init failed ({e:#}); screen scan disabled");
                return;
            }
        };
        let interval = Duration::from_millis(cfg.scan.interval_ms.max(200));
        let mut gate = Gate::default();
        let mut blind_since: Option<Instant> = None;
        loop {
            thread::sleep(interval);

            let observed = panic::catch_unwind(AssertUnwindSafe(|| {
                let shot = grab(&cfg)?;

                let title = title_crop(&shot, &cfg);
                let detected = ocr.text(&title).map(|t| title_detected(&t)).unwrap_or(false);
                Some((frame_hash(&title), detected))
            }));
            let observed = match observed {
                Ok(o) => o,
                Err(_) => {
                    eprintln!("{DIM}scanner: a scan panicked; continuing{RESET}");
                    continue;
                }
            };

            match observed {
                None => {
                    let since = blind_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_secs(30) {
                        eprintln!(
                            "{DIM}scanner: no capturable \"{}\" window for 30s; \
                             scan idle (EE.log and manual triggers still work){RESET}",
                            cfg.window_title
                        );
                        blind_since = Some(Instant::now());
                    }
                }
                Some((hash, detected)) => {
                    blind_since = None;
                    for note in gate.step(hash, detected, Instant::now()) {
                        eprintln!("{DIM}scanner: {note}{RESET}");
                    }
                    if gate.take_fire() && tx.send(Trigger::Scan).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

#[derive(Default)]
struct Gate {
    hot: bool,
    misses: u32,
    frozen: u32,
    hot_since: Option<Instant>,
    last_hash: Option<u64>,
    fire: bool,
}

const HOT_TIMEOUT: Duration = Duration::from_secs(25);

const FROZEN_LIMIT: u32 = 3;

impl Gate {
    fn step(&mut self, hash: u64, detected: bool, now: Instant) -> Vec<String> {
        let mut notes = Vec::new();
        if self.last_hash == Some(hash) {
            self.frozen += 1;
        } else {
            self.frozen = 0;
        }
        self.last_hash = Some(hash);

        if self.frozen >= FROZEN_LIMIT {
            if self.hot {
                notes.push("capture frozen; re-arming so later screens still trigger".into());
                self.disarm();
            }
            return notes;
        }
        if self.hot && self.hot_since.is_some_and(|t| now.duration_since(t) > HOT_TIMEOUT) {
            notes.push("armed too long; re-arming".into());
            self.disarm();
        }

        if detected {
            if !self.hot {
                self.fire = true;
                self.hot = true;
                self.hot_since = Some(now);
            }
            self.misses = 0;
        } else {
            self.misses += 1;

            if self.misses >= 2 {
                self.disarm();
            }
        }
        notes
    }

    fn disarm(&mut self) {
        self.hot = false;
        self.misses = 0;
        self.hot_since = None;
    }

    fn take_fire(&mut self) -> bool {
        std::mem::take(&mut self.fire)
    }
}

fn frame_hash(img: &image::RgbImage) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in img.as_raw().iter().step_by(7) {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn probe(ocr: &Ocr, shot: &image::RgbaImage, cfg: &Config) -> (bool, String) {
    let title = title_crop(shot, cfg);
    let raw = ocr.text(&title).unwrap_or_default();
    (title_detected(&raw), raw)
}

fn title_crop(shot: &image::RgbaImage, cfg: &Config) -> image::RgbImage {
    let title = capture::crop_band(shot, &cfg.scan.region, false);
    let (w, h) = title.dimensions();
    imageops::resize(&title, w * 2, h * 2, FilterType::CatmullRom)
}

fn grab(cfg: &Config) -> Option<image::RgbaImage> {
    if let Some(path) = std::env::var_os("RELIC_CHECK_FAKE_SHOT") {
        return image::open(&path).ok().map(|i| i.to_rgba8());
    }
    capture::capture_window(&cfg.window_title)
}

pub fn title_detected(raw: &str) -> bool {
    let text: Vec<char> = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let text: String = text.into_iter().collect();

    let Some(end) = (0..text.len()).find_map(|i| {
        [7usize, 6, 8].iter().find_map(|&len| {
            let window = text.get(i..i + len)?;
            (strsim::levenshtein(window, "FISSURE") <= 1).then_some(i + len)
        })
    }) else {
        return false;
    };
    let tail: String = text[end..].chars().take(8).collect();

    strsim::levenshtein(&tail, "SELECT") > 3
}

#[cfg(test)]
mod gate_tests {
    use super::{Gate, HOT_TIMEOUT};
    use std::time::{Duration, Instant};

    fn run(gate: &mut Gate, start: Instant, titles: &[bool]) -> usize {
        let mut fires = 0;
        for (i, &title) in titles.iter().enumerate() {
            gate.step(i as u64, title, start + Duration::from_secs(i as u64));
            if gate.take_fire() {
                fires += 1;
            }
        }
        fires
    }

    #[test]
    fn fires_once_per_reward_screen() {
        let mut gate = Gate::default();

        let seen = [false, true, true, true, false, false, false, true, true];
        assert_eq!(run(&mut gate, Instant::now(), &seen), 2);
    }

    #[test]
    fn single_flaky_read_does_not_refire() {
        let mut gate = Gate::default();

        let seen = [true, true, false, true, true];
        assert_eq!(run(&mut gate, Instant::now(), &seen), 1);
    }

    #[test]
    fn frozen_capture_does_not_wedge_the_gate() {
        let mut gate = Gate::default();
        let t = Instant::now();
        gate.step(1, true, t);
        assert!(gate.take_fire(), "first screen fires");

        for i in 0..10 {
            gate.step(7, true, t + Duration::from_secs(i));
            assert!(!gate.take_fire(), "must not fire on a frozen frame");
        }

        gate.step(99, true, t + Duration::from_secs(20));
        assert!(gate.take_fire(), "next real screen must still trigger");
    }

    #[test]
    fn hot_state_times_out() {
        let mut gate = Gate::default();
        let t = Instant::now();
        gate.step(1, true, t);
        assert!(gate.take_fire());

        gate.step(2, true, t + HOT_TIMEOUT + Duration::from_secs(1));
        assert!(gate.take_fire(), "re-arms rather than staying stuck hot");
    }
}

#[cfg(test)]
mod tests {
    use super::title_detected;

    #[test]
    fn detects_real_ocr_reads() {
        assert!(title_detected("EVOID FISSUREREWARDS"));
        assert!(title_detected("DFISSUREREWRUO"));
        assert!(title_detected("MVUIU FISSUREREWYRUS"));
        assert!(title_detected("V O I D  F I S S U R E / R E W A R D S"));
        assert!(title_detected("F S S CAODFISSUREREWARDS O OVOD FI"));
        assert!(title_detected("0 FISSURERE EVOD"));
        assert!(title_detected("B DIVOIDFISSURERE M PL A E 1"));
        assert!(title_detected("D VOID FISSURE/REWA O S ?"));
        assert!(title_detected("T EVOID FISSURE/REWARDSS"));
        assert!(title_detected("Onn 0n 1 +VUID FISSURE/REWARDST"));
        assert!(title_detected("M UFISSURE/REWARDS  +"));
        assert!(title_detected("OFn +  VUUFSSUREREWARDS"));
    }

    #[test]
    fn rejects_relic_selection_screen() {
        assert!(!title_detected("0On +  VUUFISSURE/SELECERE"));
        assert!(!title_detected("01 EOD MVUIUFISSURE/SEL S +"));
        assert!(!title_detected("nnrOOn +IDFISSURE/SELECTR TVUIUINOUNL"));
        assert!(!title_detected("+VOID FISSURE/SELECTR i"));
    }

    #[test]
    fn ignores_other_screens() {
        assert!(!title_detected(""));
        assert!(!title_detected("BURSTON PRIME RECEIVER"));
        assert!(!title_detected("MISSION PROGRESS"));
        assert!(!title_detected("FOCUS EARNED 10961"));
        assert!(!title_detected("O OO S"));
        assert!(!title_detected("140NG W"));
    }
}
