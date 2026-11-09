//! Offline spool: batches that could not be sent, one file per batch, in
//! `<log dir>/spool/`. Used only by the sender and writer threads.
//!
//! Each file is the exact request body of `POST /api/v1/events`, with the
//! session id and client of the session that made it. File names sort by
//! time: `<unix ms, 13 digits>-<session id, 8>-<first seq, 10 digits>-<events>.json`.
//! The total size has a cap; when it is passed, the oldest files go first
//! and their events are counted as dropped.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub struct Spool {
    dir: PathBuf,
    max_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpoolCount {
    pub files: u64,
    pub events: u64,
    pub bytes: u64,
}

struct Entry {
    path: PathBuf,
    events: u64,
    bytes: u64,
}

impl Spool {
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64) -> Spool {
        Spool {
            dir: dir.into(),
            max_bytes,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Spool files, oldest first.
    fn entries(&self) -> Vec<Entry> {
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut v: Vec<Entry> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let events = parse_name(&name)?;
                let bytes = e.metadata().ok()?.len();
                Some(Entry {
                    path: e.path(),
                    events,
                    bytes,
                })
            })
            .collect();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }

    pub fn count(&self) -> SpoolCount {
        let mut c = SpoolCount::default();
        for e in self.entries() {
            c.files += 1;
            c.events += e.events;
            c.bytes += e.bytes;
        }
        c
    }

    pub fn is_empty(&self) -> bool {
        self.entries().is_empty()
    }

    /// Write one batch body. Then apply the size cap. Returns the number of
    /// events dropped by the cap (old files deleted).
    pub fn write(
        &self,
        body: &str,
        session_id: &str,
        first_seq: u64,
        events: u64,
    ) -> io::Result<u64> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        let sid: String = session_id
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .take(8)
            .collect();
        let name = format!(
            "{:013}-{sid:0<8}-{first_seq:010}-{events}.json",
            crate::eventlog::unix_ms_now().max(0)
        );
        let path = self.dir.join(&name);
        let tmp = self
            .dir
            .join(format!(".{}.tmp", crate::identity::random_id()));
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        let r = f.write_all(body.as_bytes());
        drop(f);
        if let Err(e) = r.and_then(|_| fs::rename(&tmp, &path)) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(self.enforce_cap(&path))
    }

    /// Delete the oldest files until the total is within the cap. The file
    /// just written is kept, even if it alone is over the cap.
    fn enforce_cap(&self, keep: &Path) -> u64 {
        let entries = self.entries();
        let mut total: u64 = entries.iter().map(|e| e.bytes).sum();
        let mut dropped = 0;
        for e in entries {
            if total <= self.max_bytes {
                break;
            }
            if e.path == keep {
                continue;
            }
            if fs::remove_file(&e.path).is_ok() {
                total -= e.bytes;
                dropped += e.events;
            }
        }
        dropped
    }

    /// The oldest batch: (path, body, events).
    pub fn oldest(&self) -> Option<(PathBuf, String, u64)> {
        for e in self.entries() {
            match fs::read_to_string(&e.path) {
                Ok(body) => return Some((e.path, body, e.events)),
                // Unreadable: move it out of the way so it does not block
                // the queue forever.
                Err(_) => {
                    let _ = fs::rename(&e.path, e.path.with_extension("bad"));
                }
            }
        }
        None
    }

    pub fn remove(&self, path: &Path) {
        let _ = fs::remove_file(path);
    }
}

/// Events in a spool file, from its name. None = not a spool file.
fn parse_name(name: &str) -> Option<u64> {
    let stem = name.strip_suffix(".json")?;
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() != 4 || parts[0].len() != 13 || !parts[0].bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    parts[3].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fr-spool-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn names_parse() {
        assert_eq!(
            parse_name("1762610587215-b6d0c1e2-0000000007-12.json"),
            Some(12)
        );
        assert_eq!(parse_name("1762610587215-b6d0c1e2-0000000007-12.tmp"), None);
        assert_eq!(parse_name(".abc.tmp"), None);
        assert_eq!(parse_name("notes.json"), None);
    }

    #[test]
    fn oldest_first_and_remove() {
        let d = tmp("order");
        let s = Spool::new(&d, 1 << 20);
        assert!(s.oldest().is_none());
        s.write("{\"a\":1}", "aaaaaaaa11", 1, 3).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        s.write("{\"b\":2}", "bbbbbbbb22", 1, 2).unwrap();
        assert_eq!(
            s.count(),
            SpoolCount {
                files: 2,
                events: 5,
                bytes: 14
            }
        );
        let (p, body, n) = s.oldest().unwrap();
        assert_eq!((body.as_str(), n), ("{\"a\":1}", 3));
        s.remove(&p);
        let (_, body, _) = s.oldest().unwrap();
        assert_eq!(body, "{\"b\":2}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn cap_drops_oldest_files() {
        let d = tmp("cap");
        let s = Spool::new(&d, 250);
        let body = "x".repeat(100);
        let mut dropped = 0;
        for i in 0..5 {
            dropped += s.write(&body, "cccccccc", i * 10 + 1, 10).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let c = s.count();
        assert_eq!(c.files, 2, "{c:?}");
        assert!(c.bytes <= 250);
        assert_eq!(dropped, 30);
        // The newest two are kept.
        let (p, _, _) = s.oldest().unwrap();
        assert!(p.to_string_lossy().contains("-0000000031-"), "{p:?}");
        let _ = fs::remove_dir_all(&d);
    }
}
