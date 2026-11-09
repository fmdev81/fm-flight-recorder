//! Daily totals, retention (30 days of events, 730 days of totals), time
//! zone day boundaries, DST days, and aggregates that add up.

use chrono::NaiveDate;
use fm_flight_recorder::wire::ClientInfo;
use fr_server::config::SettingDefaults;
use fr_server::db::{self, Db, Settings};
use fr_server::query::{self, by_hour, facts, group, Agg, Filter, Kind, Range};
use fr_server::rollup;
use fr_server::store::{store_batch, Batch, Meta};
use fr_server::tz::{iso, parse_ts, Zone, DAY_MS, HOUR_MS};
use serde_json::json;

fn zone() -> Zone {
    Zone::new("America/Chicago").unwrap()
}

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn client() -> ClientInfo {
    ClientInfo {
        install_id: "00112233445566778899aabbccddeeff".into(),
        name: "Test Mac".into(),
        plugin: "0.1.0".into(),
        ..Default::default()
    }
}

/// (ts ms, duration, wait, layout, account, kind)
type Ev<'a> = (i64, u64, u64, &'a str, &'a str, &'a str);

fn store(db: &mut Db, sid: &str, evs: &[Ev], now: i64) {
    let events: Vec<_> = evs
        .iter()
        .enumerate()
        .map(|(i, (ts, dur, wait, layout, acct, kind))| {
            let ctx = json!({"file": "Invoices", "layout": layout, "account": acct, "user": acct.to_uppercase()});
            json!({"v": 1, "session_id": sid, "seq": i + 1, "type": kind, "ts": iso(*ts),
                   "end_ts": iso(*ts + *dur as i64), "duration_ms": dur, "level": "user_not_idle",
                   "before": ctx, "net": {"round_trips": 3, "tx_bytes": 1, "rx_bytes": 1, "tx_packets": 1,
                   "rx_packets": 1, "srtt_ms": 5, "wait_ms": wait, "connected": true}})
        })
        .collect();
    let b = Batch {
        v: 1,
        session_id: sid.into(),
        client: client(),
        events,
    };
    let z = zone();
    let m = Meta {
        ip: "10.0.0.2",
        seen_ms: now,
        zone: z,
        cutoff_day: rollup::cutoff_day(z, 30, now),
    };
    store_batch(db, &b, m).unwrap();
}

fn settings() -> Settings {
    Settings::load(&db::open_memory(), &SettingDefaults::default())
}

fn n(db: &Db, sql: &str) -> i64 {
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn events_older_than_30_days_go_after_their_day_is_rolled_up() {
    let mut db = db::open_memory();
    let z = zone();
    let now = parse_ts("2026-11-09T18:00:00Z").unwrap(); // noon in Chicago
    let old = parse_ts("2026-10-05T15:00:00Z").unwrap(); // 35 days ago
    let recent = parse_ts("2026-11-02T15:00:00Z").unwrap(); // 7 days ago
                                                            // Stored when they were new (so their days are dirty).
    store(
        &mut db,
        &"a".repeat(32),
        &[
            (old, 1000, 400, "Invoices | Detail", "amy", "freeze"),
            (
                old + 60_000,
                3000,
                3500,
                "Invoices | Detail",
                "amy",
                "freeze",
            ),
            (
                old + 120_000,
                9000,
                0,
                "Customers | List",
                "ben",
                "script_busy",
            ),
        ],
        old + 200_000,
    );
    store(
        &mut db,
        &"b".repeat(32),
        &[(recent, 2000, 1000, "Customers | List", "ben", "freeze")],
        recent + 1,
    );
    let s = settings();
    assert_eq!((s.event_days, s.total_days), (30, 730));
    let rep = rollup::run(&mut db, z, &s, now).unwrap();
    assert_eq!(rep.days_rolled, 2);
    assert_eq!(rep.events_deleted, 3);
    assert_eq!(n(&db, "SELECT count(*) FROM events"), 1);
    // The old day survives as totals; wait is clamped to each duration.
    let (cnt, total, wait): (i64, i64, i64) = db
        .query_row(
            "SELECT sum(n), sum(total_ms), sum(wait_ms) FROM daily_totals WHERE day = '2026-10-05' AND kind = 'freeze'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((cnt, total, wait), (2, 4000, 3400));
    assert_eq!(
        n(
            &db,
            "SELECT sum(n) FROM daily_totals WHERE kind = 'script_busy'"
        ),
        1
    );
    // A 60-day custom range reads both: totals for the old day, raw for the rest.
    let cutoff = rollup::cutoff_day(z, 30, now);
    assert_eq!(cutoff, d(2026, 10, 11));
    let r = Range::custom(z, d(2026, 9, 11), d(2026, 11, 9));
    let f = facts(&db, z, &r, cutoff, &Filter::default());
    let a = Agg::of(f.iter().filter(|x| x.kind == Kind::Freeze));
    assert_eq!((a.n, a.total, a.wait), (3, 6000, 4400));
    assert!(a.approx, "old days use the histogram");
    assert_eq!(a.max, 3000);
    // Busy time is never frozen time.
    let busy = Agg::of(f.iter().filter(|x| x.kind == Kind::Busy));
    assert_eq!((busy.n, busy.total), (1, 9000));
}

#[test]
fn totals_older_than_730_days_are_deleted() {
    let mut db = db::open_memory();
    let z = zone();
    let now = parse_ts("2026-11-09T18:00:00Z").unwrap();
    for day in ["2024-11-01", "2024-11-10", "2026-01-01"] {
        db.execute(
            "INSERT INTO daily_totals(day, kind, file, layout, script, account, client_id, n, total_ms,
               wait_ms, max_ms, rt_sum, rt_max, hist) VALUES (?1, 'freeze', 'F', 'L', '', 'a', 1, 1, 1, 0, 1, 0, 0, '[]')",
            [day],
        )
        .unwrap();
    }
    let rep = rollup::run(&mut db, z, &settings(), now).unwrap();
    // 730 days including today: the oldest kept day is 2024-11-10.
    assert_eq!(rep.totals_deleted, 1);
    assert_eq!(
        n(&db, "SELECT min(day) = '2024-11-10' FROM daily_totals"),
        1
    );
}

#[test]
fn days_follow_the_server_zone() {
    let mut db = db::open_memory();
    let z = zone();
    // 05:30Z on Nov 9 is 23:30 on Nov 8 in Chicago.
    let t = parse_ts("2026-11-09T05:30:00Z").unwrap();
    store(
        &mut db,
        &"c".repeat(32),
        &[(t, 1500, 0, "L", "amy", "freeze")],
        t + 2000,
    );
    let now = parse_ts("2026-11-10T18:00:00Z").unwrap();
    rollup::run(&mut db, z, &settings(), now).unwrap();
    assert_eq!(
        n(
            &db,
            "SELECT count(*) FROM daily_totals WHERE day = '2026-11-08'"
        ),
        1
    );
    assert_eq!(
        n(
            &db,
            "SELECT count(*) FROM daily_totals WHERE day = '2026-11-09'"
        ),
        0
    );
    // "Today" on Nov 8 (Chicago) holds it; Nov 9 does not.
    let cutoff = rollup::cutoff_day(z, 30, now);
    let today8 = Range::today(z, parse_ts("2026-11-08T20:00:00Z").unwrap());
    let today9 = Range::today(z, parse_ts("2026-11-09T20:00:00Z").unwrap());
    assert_eq!(facts(&db, z, &today8, cutoff, &Filter::default()).len(), 1);
    assert_eq!(facts(&db, z, &today9, cutoff, &Filter::default()).len(), 0);
}

#[test]
fn hours_on_dst_days() {
    let mut db = db::open_memory();
    let z = zone();
    // Nov 1 2026: DST ends in Chicago, the day has 25 hours.
    let day = d(2026, 11, 1);
    let start = z.day_start(day);
    let end = z.day_end(day);
    assert_eq!((end - start) / HOUR_MS, 25);
    store(
        &mut db,
        &"d".repeat(32),
        &[
            (start + 30 * 60_000, 1000, 0, "L", "amy", "freeze"),
            (end - 30 * 60_000, 1000, 0, "L", "amy", "freeze"),
        ],
        end,
    );
    let now = end - 1;
    let r = Range::today(z, now);
    let f = facts(
        &db,
        z,
        &r,
        rollup::cutoff_day(z, 30, now),
        &Filter::default(),
    );
    let bars = by_hour(&f, z, &r, now);
    assert_eq!(bars.len(), 25);
    assert_eq!(bars.first().unwrap().n, 1);
    assert_eq!(bars.last().unwrap().n, 1);
    assert_eq!(bars.iter().filter(|b| b.label == "01").count(), 2);
    // Mar 8 2026: DST starts, the day has 23 hours.
    let s = z.day_start(d(2026, 3, 8));
    let e = z.day_end(d(2026, 3, 8));
    assert_eq!(z.hour_starts(s, e).len(), 23);
    // The 7-day range across the change has 7 days and 167 or 169 hours.
    let w = Range::week(z, z.day_start(d(2026, 11, 4)) + 3_600_000);
    assert_eq!(w.days().len(), 7);
    assert_eq!((w.end_ms - w.start_ms) / HOUR_MS, 7 * 24 + 1);
    let _ = DAY_MS;
}

#[test]
fn a_late_event_past_the_cutoff_does_not_wipe_the_totals() {
    let mut db = db::open_memory();
    let z = zone();
    let old = parse_ts("2026-10-01T15:00:00Z").unwrap();
    store(
        &mut db,
        &"e".repeat(32),
        &[(old, 1000, 0, "L", "amy", "freeze")],
        old + 5000,
    );
    let now = parse_ts("2026-11-09T18:00:00Z").unwrap();
    rollup::run(&mut db, z, &settings(), now).unwrap();
    assert_eq!(
        n(
            &db,
            "SELECT sum(n) FROM daily_totals WHERE day = '2026-10-01'"
        ),
        1
    );
    // The plug-in sends one more event of that day from its spool, 39 days late.
    store(
        &mut db,
        &"f".repeat(32),
        &[(old + 1000, 2000, 0, "L", "amy", "freeze")],
        now,
    );
    rollup::run(&mut db, z, &settings(), now).unwrap();
    assert_eq!(
        n(
            &db,
            "SELECT sum(n) FROM daily_totals WHERE day = '2026-10-01'"
        ),
        1
    );
}

#[test]
fn rows_add_up_to_the_totals() {
    let mut db = db::open_memory();
    let z = zone();
    let now = parse_ts("2026-11-09T22:00:00Z").unwrap();
    let base = parse_ts("2026-11-09T15:00:00Z").unwrap();
    let layouts = ["A", "B", "C"];
    let accts = ["amy", "ben", ""];
    let evs: Vec<Ev> = (0..60)
        .map(|i| {
            (
                base + i * 61_000,
                300 + (i as u64 * 137) % 5000,
                (i as u64 * 97) % 6000, // sometimes more than the duration
                layouts[i as usize % 3],
                accts[i as usize % 3],
                if i % 10 == 0 { "script_busy" } else { "freeze" },
            )
        })
        .collect();
    store(&mut db, &"g".repeat(32), &evs, now);
    let r = Range::today(z, now);
    let f = facts(
        &db,
        z,
        &r,
        rollup::cutoff_day(z, 30, now),
        &Filter::default(),
    );
    let all = Agg::of(f.iter().filter(|x| x.kind == Kind::Freeze));
    for key in [0, 1, 2] {
        let gs = group(&f, |x| match key {
            0 => (x.file.clone(), x.layout.clone()),
            1 => (String::new(), x.account.clone()),
            _ => (x.file.clone(), x.script.clone()),
        });
        let n: u64 = gs.iter().map(|g| g.freeze.n).sum();
        let t: u64 = gs.iter().map(|g| g.freeze.total).sum();
        let w: u64 = gs.iter().map(|g| g.freeze.wait).sum();
        let busy: u64 = gs.iter().map(|g| g.busy.n).sum();
        assert_eq!((n, t, w), (all.n, all.total, all.wait));
        assert_eq!(busy, 6);
    }
    assert_eq!(all.n, 54);
    assert!(all.wait <= all.total);
    assert_eq!(all.client() + all.wait, all.total);
    // The same day as daily totals adds up to the same numbers.
    rollup::rollup_day(&mut db, z, z.date_of(now)).unwrap();
    let (tn, tt, tw): (i64, i64, i64) = db
        .query_row(
            "SELECT sum(n), sum(total_ms), sum(wait_ms) FROM daily_totals WHERE kind = 'freeze'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (tn as u64, tt as u64, tw as u64),
        (all.n, all.total, all.wait)
    );
    // The by-hour series adds up too.
    let bars = by_hour(&f, z, &r, now);
    assert_eq!(bars.iter().map(|b| b.n).sum::<u64>(), all.n);
    assert_eq!(bars.iter().map(|b| b.total).sum::<u64>(), all.total);
    // Users are keyed on the account; the user name is carried beside it.
    let users = group(&f, |x| (String::new(), x.account.clone()));
    let amy = users.iter().find(|g| g.name == "amy").unwrap();
    assert_eq!(amy.user, "AMY");
}

#[test]
fn compare_ranges() {
    let z = zone();
    let now = parse_ts("2026-11-09T18:00:00Z").unwrap();
    let t = Range::today(z, now);
    let p = t.previous(z, now);
    assert_eq!(p.from, d(2026, 11, 2));
    assert_eq!(
        p.end_ms,
        now - 7 * DAY_MS + 1,
        "today compares up to the same time"
    );
    let w = Range::week(z, now);
    let pw = w.previous(z, now);
    assert_eq!((pw.from, pw.to), (d(2026, 10, 27), d(2026, 11, 2)));
    let c = Range::custom(z, d(2026, 11, 1), d(2026, 11, 3));
    let pc = c.previous(z, now);
    assert_eq!((pc.from, pc.to), (d(2026, 10, 29), d(2026, 10, 31)));
}

#[test]
fn flags_use_the_thresholds() {
    let mut db = db::open_memory();
    let z = zone();
    let now = parse_ts("2026-11-09T22:00:00Z").unwrap();
    let base = parse_ts("2026-11-09T15:00:00Z").unwrap();
    let evs: Vec<Ev> = (0..20)
        .map(|i| (base + i * 1000, 9000, 0, "Slow", "amy", "freeze"))
        .collect();
    store(&mut db, &"h".repeat(32), &evs, now - 45 * 60_000);
    let mut s = settings();
    s.flag_plugin_min = "0.2.0".into();
    let r = Range::today(z, now);
    let f = facts(
        &db,
        z,
        &r,
        rollup::cutoff_day(z, 30, now),
        &Filter::default(),
    );
    let cl = query::clients(&db, now, 120_000);
    let fl = query::flags(&f, &cl, &s, now);
    assert_eq!(fl.p95.len(), 1);
    assert_eq!(fl.p95[0].1, "Slow");
    assert_eq!(fl.quiet.len(), 1, "quiet 45 min > 30 min");
    assert_eq!(fl.quiet[0].2, 45);
    assert_eq!(fl.old.len(), 1, "0.1.0 < 0.2.0");
    s.flag_p95_ms = 10_000;
    s.flag_quiet_min = 60;
    s.flag_plugin_min = "0.1.0".into();
    assert_eq!(query::flags(&f, &cl, &s, now).count(), 0);
}
