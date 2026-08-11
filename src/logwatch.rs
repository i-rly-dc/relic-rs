use crate::Trigger;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const MARKER: &str = "Got rewards";

const POLL: Duration = Duration::from_millis(200);

pub fn spawn(path: PathBuf, tx: Sender<Trigger>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut warned_missing = false;

        let mut seek_to_end = true;
        loop {
            let mut file = match File::open(&path) {
                Ok(f) => f,
                Err(e) => {
                    if !warned_missing {
                        eprintln!(
                            "log watcher: cannot open {} ({e}); will keep retrying — \
                             use F12/Enter to trigger manually",
                            path.display()
                        );
                        warned_missing = true;
                    }
                    thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };
            warned_missing = false;

            let mut pos = if seek_to_end {
                file.seek(SeekFrom::End(0)).unwrap_or(0)
            } else {
                0
            };
            seek_to_end = false;

            let mut pending: Vec<u8> = Vec::new();
            'tail: loop {
                thread::sleep(POLL);
                let len = match file.metadata() {
                    Ok(m) => m.len(),
                    Err(_) => break 'tail,
                };
                if len < pos {
                    break 'tail;
                }
                if len == pos {
                    continue;
                }
                let mut chunk = Vec::with_capacity((len - pos) as usize);
                if file.seek(SeekFrom::Start(pos)).is_err()
                    || (&mut file)
                        .take(len - pos)
                        .read_to_end(&mut chunk)
                        .is_err()
                {
                    break 'tail;
                }
                pos += chunk.len() as u64;
                pending.extend_from_slice(&chunk);

                while let Some(nl) = pending.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=nl).collect();
                    let line = String::from_utf8_lossy(&line);
                    if line.contains(MARKER) && tx.send(Trigger::Log).is_err() {
                        return;
                    }
                }
            }
        }
    })
}
