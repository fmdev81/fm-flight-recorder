//! Wire types of the ingest API (docs/ingest-api.md).
//!
//! These types are the contract between the plug-in and the companion
//! server. They depend only on serde. The server (`server/`) uses them
//! from this crate.

use crate::eventlog::Event;
use serde::{Deserialize, Serialize};

/// Version of the request envelope (and of the `/api/v1/` path).
pub const API_VERSION: u32 = 1;
/// Server limit: events in one batch.
pub const MAX_BATCH_EVENTS: usize = 500;
/// Server limit: bytes in one request body.
pub const MAX_BATCH_BYTES: usize = 1 << 20;

pub const PATH_EVENTS: &str = "/api/v1/events";
pub const PATH_PING: &str = "/api/v1/ping";
pub const PATH_HEALTH: &str = "/api/v1/health";

/// One line of the JSONL log, and one item of `EventBatch::events`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventLine {
    pub v: u32,
    /// Empty in logs written before the ingest API.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    /// 0 in logs written before the ingest API.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seq: u64,
    #[serde(flatten)]
    pub event: Event,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    /// 32 hex characters, made once and kept on disk.
    #[serde(default)]
    pub install_id: String,
    /// `client=` option, else the macOS computer name.
    #[serde(default)]
    pub name: String,
    /// Host name.
    #[serde(default)]
    pub machine: String,
    #[serde(default)]
    pub os: String,
    /// FileMaker version text.
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub plugin: String,
}

/// Body of `POST /api/v1/events`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventBatch {
    pub v: u32,
    pub session_id: String,
    pub client: ClientInfo,
    pub events: Vec<EventLine>,
}

/// Body of `POST /api/v1/ping`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ping {
    pub v: u32,
    pub session_id: String,
    pub client: ClientInfo,
    pub ts: String,
    pub running: bool,
    pub heartbeats: u64,
    pub idle_messages: u64,
    pub freezes: u64,
    #[serde(default)]
    pub last_seq: u64,
}

/// `200` reply to `POST /api/v1/events`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestReply {
    pub accepted: u64,
    pub duplicates: u64,
}

/// `4xx` reply body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorReply {
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ```json blocks of docs/ingest-api.md, in order.
    fn doc_examples() -> Vec<String> {
        let doc = include_str!("../docs/ingest-api.md");
        doc.split("```json\n")
            .skip(1)
            .map(|b| b.split("```").next().unwrap().to_string())
            .collect()
    }

    #[test]
    fn doc_batch_example_parses() {
        let ex = doc_examples();
        let b: EventBatch = serde_json::from_str(&ex[0]).expect("batch example");
        assert_eq!(b.v, API_VERSION);
        assert_eq!(b.client.name, "Front Desk iMac");
        assert_eq!(b.events.len(), 2);
        assert_eq!(b.events[0].seq, 7);
        assert_eq!(b.events[0].session_id, b.session_id);
        assert!(matches!(
            b.events[0].event,
            Event::Freeze {
                duration_ms: 2001,
                ..
            }
        ));
        assert!(
            matches!(&b.events[1].event, Event::Mark { label, .. } if label == "go to Invoices")
        );
    }

    #[test]
    fn doc_ping_example_parses() {
        let ex = doc_examples();
        let p: Ping = serde_json::from_str(&ex[1]).expect("ping example");
        assert_eq!(p.freezes, 4);
        assert_eq!(p.last_seq, 8);
        assert_eq!(p.client.install_id.len(), 32);
    }

    #[test]
    fn event_line_round_trips_and_reads_old_lines() {
        let line = EventLine {
            v: 1,
            session_id: "abc".into(),
            seq: 3,
            event: Event::SessionStop {
                ts: "2026-11-08T14:03:07.215Z".into(),
                freezes: 1,
                heartbeats: 2,
                idle_messages: 3,
            },
        };
        let s = serde_json::to_string(&line).unwrap();
        assert!(
            s.starts_with("{\"v\":1,\"session_id\":\"abc\",\"seq\":3,\"type\":\"session_stop\""),
            "{s}"
        );
        assert_eq!(serde_json::from_str::<EventLine>(&s).unwrap(), line);

        // A v1 line from before the ingest API: no session_id, no seq.
        let old = r#"{"v":1,"type":"session_stop","ts":"2026-11-08T14:03:07.215Z","freezes":1,"heartbeats":2}"#;
        let l: EventLine = serde_json::from_str(old).unwrap();
        assert_eq!((l.seq, l.session_id.as_str()), (0, ""));
    }

    #[test]
    fn replies_parse() {
        let r: IngestReply = serde_json::from_str(r#"{"accepted": 2, "duplicates": 0}"#).unwrap();
        assert_eq!(r.accepted, 2);
        let e: ErrorReply = serde_json::from_str(r#"{"error": "bad token"}"#).unwrap();
        assert_eq!(e.error, "bad token");
    }
}
