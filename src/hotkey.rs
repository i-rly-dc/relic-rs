use crate::Trigger;
use evdev::{EventSummary, KeyCode};
use std::io::BufRead;
use std::sync::mpsc::Sender;
use std::thread;

pub fn spawn_stdin(tx: Sender<Trigger>) {
    thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let msg = match line.trim() {
                "q" | "quit" | "exit" => Trigger::Quit,
                _ => Trigger::Manual,
            };
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
}

pub fn spawn_f12(tx: &Sender<Trigger>) -> usize {
    let mut hooked = 0;
    for (path, device) in evdev::enumerate() {
        let has_f12 = device
            .supported_keys()
            .is_some_and(|keys| keys.contains(KeyCode::KEY_F12));
        if !has_f12 {
            continue;
        }
        hooked += 1;
        let tx = tx.clone();
        thread::spawn(move || {
            let mut device = device;
            loop {
                let events = match device.fetch_events() {
                    Ok(ev) => ev,
                    Err(e) => {
                        eprintln!("hotkey: lost {} ({e})", path.display());
                        return;
                    }
                };
                for event in events {
                    if let EventSummary::Key(_, KeyCode::KEY_F12, 1) = event.destructure()
                        && tx.send(Trigger::Hotkey).is_err()
                    {
                        return;
                    }
                }
            }
        });
    }
    hooked
}
