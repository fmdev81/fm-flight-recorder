//! Import of JSONL logs and the synthetic data generator.

use fr_server::{fake, import, rollup};

const END: &str = "2026-11-06T22:40:00Z";

fn opts() -> fake::Opts {
    fake::Opts {
        end_ms: fr_server::tz::parse_ts(END).unwrap(),
        ..Default::default()
    }
}

#[test]
fn fake_data_is_the_same_for_the_same_seed() {
    let z = fr_server::tz::Zone::new("America/Chicago").unwrap();
    let a = fake::to_jsonl(&fake::generate(&opts(), z));
    let b = fake::to_jsonl(&fake::generate(&opts(), z));
    assert_eq!(a, b);
    let mut o = opts();
    o.seed = 8;
    assert_ne!(a, fake::to_jsonl(&fake::generate(&o, z)));
    // Several clients, files and accounts; user names differ from accounts.
    for want in [
        "Inventory",
        "Invoices",
        "script_busy",
        "\"mark\"",
        "\"user\":\"A. Rivera\"",
        "\"account\":\"arivera\"",
        "\"install_id\"",
    ] {
        assert!(a.contains(want), "{want}");
    }
}

#[test]
fn import_twice_adds_nothing_and_v1_needs_a_client_name() {
    let mut db = fr_server::db::open_memory();
    let z = fr_server::tz::Zone::new("America/Chicago").unwrap();
    let now = fr_server::tz::parse_ts(END).unwrap();
    let cutoff = rollup::cutoff_day(z, 30, now);
    let text = fake::to_jsonl(&fake::generate(&opts(), z));
    let lines = text.lines().count() as u64;
    let r1 = import::import_text(&mut db, z, cutoff, &text, "fake.jsonl", None).unwrap();
    assert_eq!((r1.lines, r1.accepted, r1.duplicates), (lines, lines, 0));
    let r2 = import::import_text(&mut db, z, cutoff, &text, "fake.jsonl", None).unwrap();
    assert_eq!((r2.accepted, r2.duplicates), (0, lines));
    // A v1 log: no session_id, no seq, no client fields.
    let v1 = "{\"v\":1,\"type\":\"session_start\",\"ts\":\"2026-11-06T14:00:00.000Z\",\"pid\":1,\"app\":\"Pro 26.0.3\",\"plugin\":\"0.1.0\",\"threshold_ms\":250,\"heartbeat\":\"idle\",\"port\":5003}\n\
              {\"v\":1,\"type\":\"freeze\",\"ts\":\"2026-11-06T14:01:00.000Z\",\"end_ts\":\"2026-11-06T14:01:02.000Z\",\"duration_ms\":2000,\"level\":\"user_not_idle\",\"before\":{\"layout\":\"Old | List\",\"account\":\"old\"},\"net\":{\"round_trips\":1,\"tx_bytes\":1,\"rx_bytes\":1,\"tx_packets\":1,\"rx_packets\":1,\"srtt_ms\":1,\"connected\":true}}\n\
              not json\n";
    let e = import::import_text(&mut db, z, cutoff, v1, "flight-old.jsonl", None).unwrap_err();
    assert!(e.contains("--client"), "{e}");
    let r =
        import::import_text(&mut db, z, cutoff, v1, "flight-old.jsonl", Some("Old Mac")).unwrap();
    assert_eq!((r.accepted, r.skipped), (2, 1));
    let r =
        import::import_text(&mut db, z, cutoff, v1, "flight-old.jsonl", Some("Old Mac")).unwrap();
    assert_eq!((r.accepted, r.duplicates), (0, 2));
    let name: String = db
        .query_row("SELECT c.name FROM events e JOIN clients c ON c.id = e.client_id WHERE e.layout = 'Old | List'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "Old Mac");
}
