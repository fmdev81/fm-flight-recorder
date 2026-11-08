//! JSONL event log in ~/Library/Logs/FlightRecorder/.
//!
//! One JSON object per line. The schema is versioned (`v`) and documented in
//! README.md. Writing happens on a background thread, never on FileMaker's
//! main thread.

use crate::context::Context;
use crate::net::NetWindow;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // events are short-lived, one per freeze
pub enum Event {
    SessionStart {
        ts: String,
        pid: u32,
        app: String,
        plugin: String,
        threshold_ms: u64,
        heartbeat: String,
        port: u16,
    },
    /// The main thread did not respond for `duration_ms`.
    Freeze {
        ts: String,
        end_ts: String,
        duration_ms: u64,
        /// Idle level reported by the last heartbeat before the freeze.
        level: String,
        /// Context read at the last safe idle before the freeze.
        before: Context,
        /// Context read at the first safe idle after the freeze.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<Context>,
        net: NetWindow,
        /// FlightRecorder_Mark labels set during or just before the freeze.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        marks: Vec<String>,
    },
    /// FileMaker reported "script running" continuously for `duration_ms`.
    ScriptBusy {
        ts: String,
        end_ts: String,
        duration_ms: u64,
        before: Context,
        net: NetWindow,
    },
    Mark {
        ts: String,
        label: String,
        context: Context,
    },
    SessionStop {
        ts: String,
        freezes: u64,
        heartbeats: u64,
    },
}

#[derive(Serialize)]
struct Line<'a> {
    v: u32,
    #[serde(flatten)]
    event: &'a Event,
}

pub fn to_line(e: &Event) -> String {
    serde_json::to_string(&Line {
        v: SCHEMA_VERSION,
        event: e,
    })
    .expect("event serializes")
}

pub fn parse_line(s: &str) -> Option<Event> {
    serde_json::from_str::<Event>(s).ok()
}

/// Default directory: $FLIGHT_RECORDER_LOG_DIR, else ~/Library/Logs/FlightRecorder.
pub fn default_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FLIGHT_RECORDER_LOG_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library/Logs/FlightRecorder")
}

pub struct LogWriter {
    dir: PathBuf,
}

impl LogWriter {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// One file per day: flight-YYYY-MM-DD.jsonl
    pub fn path_for_now(&self) -> PathBuf {
        let date = &iso_now()[..10];
        self.dir.join(format!("flight-{date}.jsonl"))
    }

    pub fn write(&self, e: &Event) -> std::io::Result<PathBuf> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path_for_now();
        let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
        let mut line = to_line(e);
        line.push('\n');
        f.write_all(line.as_bytes())?;
        Ok(path)
    }
}

/// Wall clock as ISO-8601 UTC with ms, e.g. 2026-11-08T14:03:07.215Z.
pub fn iso_now() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    iso_from_unix_ms(d.as_millis() as i64)
}

pub fn unix_ms_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn iso_from_unix_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// Howard Hinnant's days -> civil date algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates() {
        assert_eq!(iso_from_unix_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            iso_from_unix_ms(1_762_610_587_215),
            "2025-11-08T14:03:07.215Z"
        );
        assert_eq!(
            iso_from_unix_ms(951_782_400_000),
            "2000-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn freeze_line_round_trips() {
        let e = Event::Freeze {
            ts: "2026-11-08T14:03:07.215Z".into(),
            end_ts: "2026-11-08T14:03:11.415Z".into(),
            duration_ms: 4200,
            level: "user_not_idle".into(),
            before: Context {
                layout: "Invoices".into(),
                script: "OnRecordLoad".into(),
                ..Default::default()
            },
            after: None,
            net: NetWindow {
                round_trips: 38,
                connected: true,
                ..Default::default()
            },
            marks: vec!["load invoice".into()],
        };
        let line = to_line(&e);
        assert!(line.starts_with("{\"v\":1,\"type\":\"freeze\""), "{line}");
        assert!(!line.contains("\"after\""));
        assert_eq!(parse_line(&line), Some(e));
    }

    #[test]
    fn writer_appends_lines() {
        let dir = std::env::temp_dir().join(format!("fr-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let w = LogWriter::new(&dir);
        let e = Event::SessionStop {
            ts: iso_now(),
            freezes: 2,
            heartbeats: 10,
        };
        let p = w.write(&e).unwrap();
        w.write(&e).unwrap();
        let text = fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("flight-"));
        let _ = fs::remove_dir_all(&dir);
    }
}
