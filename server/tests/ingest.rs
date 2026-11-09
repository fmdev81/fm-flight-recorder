//! Contract tests of the ingest API against docs/ingest-api.md, with a real
//! server on a local port. The batch and the ping are posted with the
//! plug-in's own HTTP client (`fm_flight_recorder::http`).

mod common;

use common::*;
use fm_flight_recorder::http::{post_json as plugin_post, Abort, Timeouts};
use fm_flight_recorder::recorder::Endpoint;
use fm_flight_recorder::wire::{IngestReply, MAX_BATCH_BYTES, PATH_EVENTS, PATH_PING};
use serde_json::{json, Value};
use std::time::Duration;

fn plugin(base: &str, path: &str, token: &str, body: &str) -> (u16, String) {
    let ep = Endpoint::parse(base).unwrap();
    let t = Timeouts {
        connect: Duration::from_secs(2),
        io: Duration::from_secs(5),
    };
    let r = plugin_post(&ep, path, token, body.as_bytes(), t, &Abort::default()).unwrap();
    (r.status, r.body)
}

fn reply(body: &str) -> IngestReply {
    serde_json::from_str(body).unwrap()
}

#[test]
fn good_batch_is_stored_and_a_resend_is_dropped() {
    let (app, base, _d) = server("good");
    let token = app.token();
    let batch = &doc_examples()[0];
    let (st, body) = plugin(&base, PATH_EVENTS, &token, batch);
    assert_eq!(st, 200, "{body}");
    assert_eq!(
        reply(&body),
        IngestReply {
            accepted: 2,
            duplicates: 0
        }
    );
    // The same batch again (a retry): stored once.
    let (st, body) = plugin(&base, PATH_EVENTS, &token, batch);
    assert_eq!(st, 200);
    assert_eq!(
        reply(&body),
        IngestReply {
            accepted: 0,
            duplicates: 2
        }
    );
    assert_eq!(count(&app, "SELECT count(*) FROM events"), 2);
    // The client and the session are known; last seen and IP are set.
    assert_eq!(count(&app, "SELECT count(*) FROM clients WHERE install_id = '3f9c2a7e5b0d4e1f8a6c9b2d7e4f1a03' AND ip = '127.0.0.1' AND last_seen_ms > 0"), 1);
    assert_eq!(count(&app, "SELECT freezes FROM sessions"), 1);
    assert_eq!(count(&app, "SELECT frozen_ms FROM sessions"), 2001);
    // The freeze has its context columns.
    let db = app.db.lock().unwrap();
    let (layout, wait): (String, i64) = db
        .query_row(
            "SELECT layout, wait_ms FROM events WHERE type = 'freeze'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((layout.as_str(), wait), ("Customers | List", 2001));
}

#[test]
fn missing_or_wrong_token_is_401_and_stores_nothing() {
    let (app, base, _d) = server("token");
    let batch = &doc_examples()[0];
    let r = post_json(&base, PATH_EVENTS, None, batch);
    assert_eq!(r.status, 401);
    assert_eq!(
        serde_json::from_str::<Value>(&r.body).unwrap()["error"],
        "bad token"
    );
    let (st, _) = plugin(&base, PATH_EVENTS, "not-the-token", batch);
    assert_eq!(st, 401);
    let r = post_json(&base, PATH_PING, Some("nope"), &doc_examples()[1]);
    assert_eq!(r.status, 401);
    assert_eq!(count(&app, "SELECT count(*) FROM events"), 0);
    assert_eq!(count(&app, "SELECT count(*) FROM clients"), 0);
}

#[test]
fn a_new_token_makes_the_old_one_fail() {
    let (app, base, _d) = server("newtoken");
    let old = app.token();
    let r = post_json(&base, "/api/v1/token", None, "");
    assert_eq!(r.status, 200);
    let new = serde_json::from_str::<Value>(&r.body).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(old, new);
    assert_eq!(new.len(), 32);
    let shown = get(&base, "/api/v1/token");
    assert!(shown.body.contains(&new));
    let batch = &doc_examples()[0];
    assert_eq!(plugin(&base, PATH_EVENTS, &old, batch).0, 401);
    assert_eq!(plugin(&base, PATH_EVENTS, &new, batch).0, 200);
    // The token is kept in the database.
    let db = app.db.lock().unwrap();
    assert_eq!(fr_server::db::get_setting(&db, "event_token").unwrap(), new);
}

#[test]
fn malformed_body_is_400_and_a_batch_is_all_or_nothing() {
    let (app, base, _d) = server("bad");
    let token = app.token();
    let r = post_json(&base, PATH_EVENTS, Some(&token), "{not json");
    assert_eq!(r.status, 400);
    assert!(serde_json::from_str::<Value>(&r.body).unwrap()["error"].is_string());
    // One good event and one with a wrong session_id: nothing is stored.
    let mut b: Value = serde_json::from_str(&doc_examples()[0]).unwrap();
    b["events"][1]["session_id"] = json!("ffffffffffffffffffffffffffffffff");
    let r = post_json(&base, PATH_EVENTS, Some(&token), &b.to_string());
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("events[1]"), "{}", r.body);
    // A freeze without its required fields.
    let mut b: Value = serde_json::from_str(&doc_examples()[0]).unwrap();
    b["events"][0]
        .as_object_mut()
        .unwrap()
        .remove("duration_ms");
    assert_eq!(
        post_json(&base, PATH_EVENTS, Some(&token), &b.to_string()).status,
        400
    );
    // A bad time.
    let mut b: Value = serde_json::from_str(&doc_examples()[0]).unwrap();
    b["events"][1]["ts"] = json!("yesterday");
    assert_eq!(
        post_json(&base, PATH_EVENTS, Some(&token), &b.to_string()).status,
        400
    );
    // An unknown envelope version.
    let mut b: Value = serde_json::from_str(&doc_examples()[0]).unwrap();
    b["v"] = json!(2);
    assert_eq!(
        post_json(&base, PATH_EVENTS, Some(&token), &b.to_string()).status,
        400
    );
    // seq must be 1 or more.
    let mut b: Value = serde_json::from_str(&doc_examples()[0]).unwrap();
    b["events"][0]["seq"] = json!(0);
    assert_eq!(
        post_json(&base, PATH_EVENTS, Some(&token), &b.to_string()).status,
        400
    );
    assert_eq!(count(&app, "SELECT count(*) FROM events"), 0);
    assert_eq!(count(&app, "SELECT count(*) FROM clients"), 0);
    // A malformed ping.
    assert_eq!(post_json(&base, PATH_PING, Some(&token), "[]").status, 400);
}

#[test]
fn too_many_events_or_bytes_is_413() {
    let (app, base, _d) = server("big");
    let token = app.token();
    let mut b: Value = serde_json::from_str(&doc_examples()[0]).unwrap();
    let mark = b["events"][1].clone();
    let events: Vec<Value> = (1..=501)
        .map(|i| {
            let mut m = mark.clone();
            m["seq"] = json!(i);
            m
        })
        .collect();
    b["events"] = Value::Array(events);
    let r = post_json(&base, PATH_EVENTS, Some(&token), &b.to_string());
    assert_eq!(r.status, 413);
    assert_eq!(
        serde_json::from_str::<Value>(&r.body).unwrap()["error"],
        "batch too large"
    );
    // Over 1 MiB of body.
    let mut b: Value = serde_json::from_str(&doc_examples()[0]).unwrap();
    b["events"][1]["label"] = json!("x".repeat(MAX_BATCH_BYTES));
    let r = post_json(&base, PATH_EVENTS, Some(&token), &b.to_string());
    assert_eq!(r.status, 413);
    assert_eq!(count(&app, "SELECT count(*) FROM events"), 0);
}

#[test]
fn unknown_fields_and_types_are_accepted() {
    let (app, base, _d) = server("unknown");
    let token = app.token();
    let mut b: Value = serde_json::from_str(&doc_examples()[0]).unwrap();
    b["new_envelope_field"] = json!(1);
    b["events"][0]["new_field"] = json!({"a": 1});
    let mut e = b["events"][1].clone();
    e["seq"] = json!(9);
    e["type"] = json!("gc_pause");
    e["pause_ms"] = json!(12);
    b["events"].as_array_mut().unwrap().push(e);
    let (st, body) = plugin(&base, PATH_EVENTS, &token, &b.to_string());
    assert_eq!(st, 200, "{body}");
    assert_eq!(reply(&body).accepted, 3);
    assert_eq!(
        count(&app, "SELECT count(*) FROM events WHERE type = 'gc_pause'"),
        1
    );
}

#[test]
fn ping_sets_last_seen_and_counters() {
    let (app, base, _d) = server("ping");
    let token = app.token();
    let (st, body) = plugin(&base, PATH_PING, &token, &doc_examples()[1]);
    assert_eq!(st, 200);
    assert_eq!(body.trim(), "{}");
    assert_eq!(
        count(&app, "SELECT count(*) FROM clients WHERE last_seen_ms > 0"),
        1
    );
    assert_eq!(count(&app, "SELECT heartbeats FROM sessions"), 123456);
    assert_eq!(count(&app, "SELECT last_seq FROM sessions"), 8);
    // The client counts as connected in the footer status.
    let s: Value = serde_json::from_str(&get(&base, "/api/v1/status").body).unwrap();
    assert_eq!(s["connected"], 1);
    assert_eq!(s["clients"], 1);
    assert_eq!(s["tz"], "America/Chicago");
    assert!(s["tz_offset"].as_str().unwrap().starts_with("UTC"));
}

#[test]
fn health_needs_no_token() {
    let (_app, base, _d) = server("health");
    let r = get(&base, "/api/v1/health");
    assert_eq!(r.status, 200);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
}
