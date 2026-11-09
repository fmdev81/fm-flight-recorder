//! Import JSONL logs of the plug-in (`fr-server import`).
//!
//! - Lines with `session_id` and `seq` keep their id, so a log and the same
//!   events sent by the plug-in are stored once.
//! - Older v1 lines have no id. They get a stable id from the file name, the
//!   client name and the line number, so a second import adds nothing.
//! - The client comes from the `session_start` line (newer logs), else from
//!   `--client` (v1 logs have no client fields).

use crate::db::Db;
use crate::store::{self, Batch, Meta};
use crate::tz::{parse_ts, Zone};
use chrono::NaiveDate;
use fm_flight_recorder::wire::{ClientInfo, MAX_BATCH_EVENTS};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub lines: u64,
    pub accepted: u64,
    pub duplicates: u64,
    pub skipped: u64,
}

impl std::ops::AddAssign for Report {
    fn add_assign(&mut self, o: Report) {
        self.lines += o.lines;
        self.accepted += o.accepted;
        self.duplicates += o.duplicates;
        self.skipped += o.skipped;
    }
}

/// FNV-1a, 64 bit.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn hex32(a: &str, b: &str) -> String {
    format!("{:016x}{:016x}", fnv(a), fnv(&format!("{b}\u{1}{a}")))
}

/// The client of a v1 log, from `--client`.
pub fn named_client(name: &str) -> ClientInfo {
    ClientInfo {
        install_id: hex32("import-client", name),
        name: name.to_string(),
        ..Default::default()
    }
}

fn client_of_start(v: &Value) -> Option<ClientInfo> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let c = ClientInfo {
        install_id: s("install_id"),
        name: s("client"),
        machine: s("machine"),
        os: s("os"),
        app: s("app"),
        plugin: s("plugin"),
    };
    (!c.install_id.is_empty() || !c.name.is_empty()).then_some(c)
}

struct Sess {
    id: String,
    client: Option<ClientInfo>,
    lines: Vec<Value>,
}

pub fn import_file(
    db: &mut Db,
    zone: Zone,
    cutoff: NaiveDate,
    path: &Path,
    client: Option<&str>,
) -> Result<Report, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let source = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    import_text(db, zone, cutoff, &text, &source, client)
}

pub fn import_text(
    db: &mut Db,
    zone: Zone,
    cutoff: NaiveDate,
    text: &str,
    source: &str,
    client: Option<&str>,
) -> Result<Report, String> {
    let mut rep = Report::default();
    let mut sessions: Vec<Sess> = Vec::new();
    let mut synth: Option<usize> = None; // index of the open v1 session
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        rep.lines += 1;
        let Ok(mut v) = serde_json::from_str::<Value>(line) else {
            rep.skipped += 1;
            continue;
        };
        let Some(kind) = v.get("type").and_then(Value::as_str).map(str::to_string) else {
            rep.skipped += 1;
            continue;
        };
        let sid = v
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let seq = v.get("seq").and_then(Value::as_u64).unwrap_or(0);
        let idx = if !sid.is_empty() && seq > 0 {
            match sessions.iter().position(|s| s.id == sid) {
                Some(k) => k,
                None => {
                    sessions.push(Sess {
                        id: sid,
                        client: None,
                        lines: vec![],
                    });
                    sessions.len() - 1
                }
            }
        } else {
            // v1 line: a new session at each session_start.
            if synth.is_none() || kind == "session_start" {
                let id = hex32(source, &format!("{}\u{1}{i}", client.unwrap_or("")));
                sessions.push(Sess {
                    id,
                    client: None,
                    lines: vec![],
                });
                synth = Some(sessions.len() - 1);
            }
            let k = synth.unwrap();
            if let Some(o) = v.as_object_mut() {
                o.insert("session_id".into(), Value::from(sessions[k].id.clone()));
                o.insert("seq".into(), Value::from(i as u64 + 1));
            }
            k
        };
        if let Some(o) = v.as_object_mut() {
            o.entry("v").or_insert(Value::from(1));
        }
        if kind == "session_start" {
            if let Some(c) = client_of_start(&v) {
                sessions[idx].client = Some(c);
            }
        }
        sessions[idx].lines.push(v);
    }
    for s in sessions {
        let c = match (s.client, client) {
            (Some(c), _) => c,
            (None, Some(name)) => named_client(name),
            (None, None) => {
                return Err(format!(
                    "{source}: the log has no client fields; give the client name with --client"
                ))
            }
        };
        for chunk in s.lines.chunks(MAX_BATCH_EVENTS) {
            let seen = chunk
                .iter()
                .filter_map(|v| v.get("ts").and_then(Value::as_str).and_then(parse_ts))
                .max()
                .unwrap_or(0);
            let b = Batch {
                v: 1,
                session_id: s.id.clone(),
                client: c.clone(),
                events: chunk.to_vec(),
            };
            let meta = Meta {
                ip: "",
                seen_ms: seen,
                zone,
                cutoff_day: cutoff,
            };
            // A bad line stops only its own batch; count it as skipped.
            match store::store_batch(db, &b, meta) {
                Ok(r) => {
                    rep.accepted += r.accepted;
                    rep.duplicates += r.duplicates;
                }
                Err(store::StoreError::Db(e)) => return Err(e),
                Err(_) => {
                    for v in chunk {
                        let one = Batch {
                            events: vec![v.clone()],
                            ..b_clone(&b)
                        };
                        match store::store_batch(db, &one, meta) {
                            Ok(r) => {
                                rep.accepted += r.accepted;
                                rep.duplicates += r.duplicates;
                            }
                            Err(store::StoreError::Db(e)) => return Err(e),
                            Err(_) => rep.skipped += 1,
                        }
                    }
                }
            }
        }
    }
    Ok(rep)
}

fn b_clone(b: &Batch) -> Batch {
    Batch {
        v: b.v,
        session_id: b.session_id.clone(),
        client: b.client.clone(),
        events: vec![],
    }
}
