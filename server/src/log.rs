//! A small logger: one line per message, to stdout or to a file.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static SINK: Mutex<Option<File>> = Mutex::new(None);

/// Send the log lines to `path` (appended). Without a call, lines go to stdout.
pub fn to_file(path: &Path) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let f = OpenOptions::new().create(true).append(true).open(path)?;
    *SINK.lock().unwrap() = Some(f);
    Ok(())
}

pub fn line(msg: &str) {
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
    let text = format!("{ts} {msg}\n");
    let mut g = SINK.lock().unwrap();
    match g.as_mut() {
        Some(f) => {
            let _ = f.write_all(text.as_bytes());
        }
        None => {
            let _ = std::io::stdout().write_all(text.as_bytes());
        }
    }
}

#[macro_export]
macro_rules! logln {
    ($($t:tt)*) => { $crate::log::line(&format!($($t)*)) };
}
