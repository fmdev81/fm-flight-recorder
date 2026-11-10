//! End to end: the plug-in's real recorder and sender post to a running
//! server, and the events show on the dashboard pages. Then a larger
//! synthetic data set (posted with the plug-in's HTTP client and batch
//! body) must render every page and every detail page.

mod common;

use common::*;
use fm_flight_recorder::context::Context;
use fm_flight_recorder::recorder::{Config as RecConfig, Level, Recorder};
use fr_server::{fake, rollup};
use std::thread;
use std::time::{Duration, Instant};

fn wait_for(mut f: impl FnMut() -> bool, secs: u64) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn ctx(layout: &str, script: &str) -> Context {
    Context {
        file: "E2E Orders".into(),
        layout: layout.into(),
        layout_table: "Orders".into(),
        script: script.into(),
        account: "e2e_user".into(),
        user: "E2E Person".into(),
        ..Default::default()
    }
}

/// Unescape the few entities askama writes, to search the page text.
fn text(html: &str) -> String {
    html.replace("&#38;", "&")
        .replace("&amp;", "&")
        .replace("&#124;", "|")
        .replace("&#43;", "+")
}

#[test]
fn plugin_recorder_posts_to_the_server_and_pages_show_it() {
    let (app, base, dir) = server("e2e");
    let token = app.token();
    let mut cfg = RecConfig {
        log_dir: dir.join("logs"),
        state_dir: dir.join("state"),
        net_port: None,
        app: "Pro 26.0.3".into(),
        threshold_ms: 150,
        send_interval_ms: 100,
        send_retry_ms: 100,
        send_ping_ms: 300,
        ..Default::default()
    };
    let rep = cfg.apply_options(&format!("endpoint={base}; token={token}; client=E2E Mac"));
    assert!(rep.errors.is_empty(), "{rep:?}");
    let rec = Recorder::start(cfg, Box::new(fm_flight_recorder::net::NoProbe));

    // Healthy idle with the "before" context, then a freeze, then "after".
    for i in 0..20 {
        rec.beat(Level::UserNotIdle);
        if i % 5 == 0 {
            rec.offer_context(ctx("Orders | List", "Open Order"));
        }
        thread::sleep(Duration::from_millis(10));
    }
    rec.mark("Open Order start", ctx("Orders | List", "Open Order"));
    thread::sleep(Duration::from_millis(450)); // no beats: a freeze
    for i in 0..30 {
        rec.beat(Level::UserNotIdle);
        if i % 5 == 0 {
            rec.offer_context(ctx("E2E | Detail", ""));
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        wait_for(
            || count(&app, "SELECT count(*) FROM events WHERE type = 'freeze'") >= 1,
            10
        ),
        "the freeze did not arrive"
    );
    assert!(
        wait_for(
            || count(&app, "SELECT count(*) FROM sessions WHERE heartbeats > 0") >= 1,
            10
        ),
        "no ping"
    );
    let sid = rec.session_id().to_string();
    rec.stop();
    assert!(wait_for(
        || count(
            &app,
            "SELECT count(*) FROM events WHERE type = 'session_start'"
        ) == 1,
        5
    ));

    // The stored freeze has the after layout, the before layout as "from",
    // the account, the client name and the plug-in version.
    {
        let db = app.db.lock().unwrap();
        let (layout, from, account, dur, s): (String, String, String, i64, String) = db
            .query_row(
                "SELECT layout, from_layout, account, duration_ms, session_id FROM events WHERE type = 'freeze'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            (layout.as_str(), from.as_str(), account.as_str()),
            ("E2E | Detail", "Orders | List", "e2e_user")
        );
        assert!(dur >= 300, "{dur}");
        assert_eq!(s, sid);
        let (name, plugin): (String, String) = db
            .query_row("SELECT name, plugin FROM clients", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(name, "E2E Mac");
        assert_eq!(plugin, fm_flight_recorder::VERSION);
    }

    // The dashboard pages show it.
    let ov = get(&base, "/");
    assert_eq!(ov.status, 200);
    assert!(text(&ov.body).contains("E2E | Detail"));
    let fz = get(&base, "/freezes");
    assert_eq!(fz.status, 200);
    let t = text(&fz.body);
    assert!(t.contains("1 of 1 freezes"), "freezes count");
    assert!(t.contains("e2e_user") && t.contains("E2E Mac") && t.contains("Orders | List"));
    let id = count(&app, "SELECT id FROM events WHERE type = 'freeze'");
    let fd = get(&base, &format!("/freeze?id={id}"));
    assert_eq!(fd.status, 200);
    let t = text(&fd.body);
    assert!(
        t.contains("E2E | Detail") && t.contains("Open Order start") && t.contains("E2E Person")
    );
    let cl = get(&base, "/clients");
    assert!(text(&cl.body).contains("E2E Mac"));
    let us = get(&base, "/users");
    assert!(text(&us.body).contains("e2e_user") && text(&us.body).contains("E2E Person"));
    let ly = get(&base, "/layout?file=E2E+Orders&name=E2E+%7C+Detail");
    assert_eq!(ly.status, 200);
    assert!(text(&ly.body).contains("Orders | List"), "came from");
}

const END: &str = "2026-11-06T22:40:00Z";

fn opts() -> fake::Opts {
    fake::Opts {
        end_ms: fr_server::tz::parse_ts(END).unwrap(),
        ..Default::default()
    }
}

#[test]
fn every_page_renders_with_synthetic_data() {
    let (app, base, _d) = server("pages");
    let token = app.token();
    let z = app.zone;
    let mut o = opts();
    o.end_ms = fr_server::tz::now_ms();
    let sessions = fake::generate(&o, z);
    let total: u64 = sessions.iter().map(|s| s.lines.len() as u64).sum();
    // Posted with the plug-in's HTTP client and the plug-in's batch body.
    let n = fake::post(&sessions, &base, &token, o.end_ms).unwrap();
    assert_eq!(n, total);
    assert_eq!(
        fake::post(&sessions, &base, &token, o.end_ms).unwrap(),
        0,
        "a resend adds nothing"
    );
    {
        let mut db = app.db.lock().unwrap();
        let s = app.settings(&db);
        rollup::run(&mut db, z, &s, fr_server::tz::now_ms()).unwrap();
    }
    let lists: [(&str, &[&str]); 13] = [
        (
            "/",
            &[
                "Flags",
                "Who froze, when",
                "Silent, session open",
                "Worst layouts",
                "Worst scripts",
                "Latest freezes",
            ],
        ),
        (
            "/?range=7d",
            &[
                "Who froze, when",
                "Clients affected",
                "Table view",
                "/freeze?id=",
            ],
        ),
        (
            "/fragment/lanes?range=today",
            &["Who froze, when", "hx-trigger=\"every 30s"],
        ),
        (
            "/static/licenses.txt",
            &["htmx", "uPlot", "IBM Plex", "SIL Open Font License"],
        ),
        (
            "/static/app.css",
            &["IBM Plex Sans", "IBM Plex Mono", "--link"],
        ),
        (
            "/?range=custom&from=2026-10-01&to=2026-12-01",
            &["Frozen time by day"],
        ),
        (
            "/layouts?range=7d",
            &["Layouts by total frozen time", "Avg round trips", "<tfoot>"],
        ),
        (
            "/scripts?range=7d",
            &[
                "All scripts",
                "Seen in",
                "Script busy",
                "No script name",
                "scripts=0",
            ],
        ),
        (
            "/users",
            &[
                "All accounts",
                "Shared clients",
                "Accounts on 2+ clients",
                "arivera",
                "A. Rivera",
            ],
        ),
        (
            "/clients",
            &["Who froze, when", "FileMaker Pro", "0.0.9", "Connected"],
        ),
        ("/freezes?range=7d", &["freezes", "Load more", "Clear"]),
        (
            "/setup",
            &[
                "Event token",
                "FlightRecorder_Start",
                "LAN · no sign-in",
                "Highlight only",
                "Keep daily totals",
                &token,
            ],
        ),
        ("/api/v1/flags?range=7d", &["p95", "quiet", "old"]),
    ];
    for (path, want) in lists {
        let r = get(&base, path);
        assert_eq!(r.status, 200, "{path}");
        let t = text(&r.body);
        for w in want {
            assert!(t.contains(w), "{path}: {w}");
        }
    }
    // Every detail page linked from the lists answers 200.
    let mut links = std::collections::BTreeSet::new();
    for path in [
        "/?range=7d",
        "/layouts?range=7d",
        "/scripts?range=7d",
        "/users",
        "/clients",
        "/freezes?range=7d",
    ] {
        let t = text(&get(&base, path).body);
        for part in t.split("href=\"").skip(1) {
            let l = part.split('"').next().unwrap();
            if ["/layout?", "/script?", "/user?", "/client?", "/freeze?"]
                .iter()
                .any(|p| l.starts_with(p))
            {
                links.insert(l.to_string());
            }
        }
    }
    for kind in ["/layout?", "/script?", "/user?", "/client?", "/freeze?"] {
        assert!(links.iter().any(|l| l.starts_with(kind)), "no {kind} link");
    }
    for l in &links {
        let r = get(&base, l);
        assert_eq!(r.status, 200, "{l}");
        let r = get(&base, &format!("{l}&cmp=1"));
        assert_eq!(r.status, 200, "{l}&cmp=1");
    }
    // Load more: the next page of rows and a new button (htmx partial).
    let t = text(&get(&base, "/freezes?range=7d").body);
    let more = t
        .split("hx-get=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
    let r = get(&base, &more);
    assert_eq!(r.status, 200);
    assert!(r.body.contains("<tr class=\"fr\">") && r.body.contains("fz-count"));
    let j = get(&base, "/api/v1/freezes?range=7d");
    assert_eq!(j.status, 200);
    let v: serde_json::Value = serde_json::from_str(&j.body).unwrap();
    assert_eq!(v["rows"].as_array().unwrap().len(), 100);
    // A missing freeze or client is 404, not an error.
    assert_eq!(get(&base, "/freeze?id=999999").status, 404);
    assert_eq!(get(&base, "/client?id=999999").status, 404);
    // Static files are in the binary.
    for s in [
        "/static/app.css",
        "/static/app.js",
        "/static/vendor/htmx.min.js",
        "/static/vendor/uPlot.iife.min.js",
        "/static/licenses.txt",
    ] {
        assert_eq!(get(&base, s).status, 200, "{s}");
    }
}

#[test]
fn setup_saves_the_settings_and_makes_a_new_token() {
    let (app, base, _d) = server("setup");
    let form = "flag_p95_s=6.5&flag_quiet_min=45&flag_plugin_min=0.1.0&rec_threshold_ms=300&rec_port=443&event_days=14&total_days=365";
    let r = request(
        &base,
        "POST",
        "/setup",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        form.as_bytes(),
    );
    assert_eq!(r.status, 303);
    {
        let db = app.db.lock().unwrap();
        let s = app.settings(&db);
        assert_eq!(
            (
                s.flag_p95_ms,
                s.flag_quiet_min,
                s.rec_threshold_ms,
                s.rec_port,
                s.event_days,
                s.total_days
            ),
            (6500, 45, 300, 443, 14, 365)
        );
    }
    let page = text(&get(&base, "/setup").body);
    // A form without the send fields keeps the live defaults.
    assert!(page.contains("threshold=300; port=443; send_ms=250; ping_ms=15000"), "{page}");
    let form2 = format!("{form}&rec_send_ms=1000&rec_ping_s=30");
    let r = request(
        &base,
        "POST",
        "/setup",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        form2.as_bytes(),
    );
    assert_eq!(r.status, 303);
    let page = text(&get(&base, "/setup").body);
    assert!(page.contains("port=443; send_ms=1000; ping_ms=30000"));
    for bad in ["rec_send_ms=100", "rec_ping_s=1"] {
        let f = format!("{form}&{bad}");
        let r = request(
            &base,
            "POST",
            "/setup",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            f.as_bytes(),
        );
        assert_eq!(r.status, 400, "{bad}");
    }
    let bad = request(
        &base,
        "POST",
        "/setup",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        b"flag_p95_s=abc",
    );
    assert_eq!(bad.status, 400);
    // A form or a script on another site cannot change the settings or the token.
    let old = app.token();
    let evil = [
        ("Origin", "http://evil.example"),
        ("Content-Type", "application/x-www-form-urlencoded"),
    ];
    assert_eq!(
        request(&base, "POST", "/setup", &evil, form.as_bytes()).status,
        403
    );
    assert_eq!(
        request(&base, "POST", "/setup/token", &evil, b"").status,
        403
    );
    assert_eq!(
        request(&base, "POST", "/api/v1/token", &evil, b"").status,
        403
    );
    assert_eq!(app.token(), old);
    let same = [("Origin", base.as_str())];
    let r = request(&base, "POST", "/setup/token", &same, b"");
    assert_eq!(r.status, 200);
    assert_ne!(app.token(), old);
    assert!(r.body.contains(&app.token()));
}

#[test]
fn fonts_are_in_the_binary_and_the_early_morning_shows_standby() {
    let (_app, base, _dir) = server("fonts");
    for f in [
        "IBMPlexSans-Regular-Latin1",
        "IBMPlexSans-Regular-Latin2",
        "IBMPlexSans-Regular-Pi",
        "IBMPlexSans-Medium-Latin1",
        "IBMPlexSans-Medium-Latin2",
        "IBMPlexSans-SemiBold-Latin1",
        "IBMPlexSans-SemiBold-Latin2",
        "IBMPlexMono-Regular-Latin1",
        "IBMPlexMono-Medium-Latin1",
        "IBMPlexMono-Regular-Pi",
    ] {
        let r = get(&base, &format!("/static/fonts/{f}.woff2"));
        assert_eq!(r.status, 200, "{f}");
        assert!(r.body.starts_with("wOF2"), "{f} is woff2");
    }
    assert_eq!(get(&base, "/static/fonts/other.woff2").status, 404);
    // No clients and no sessions: the recorder is on standby, the tiles say so.
    let r = get(&base, "/");
    assert_eq!(r.status, 200);
    let t = text(&r.body);
    for w in [
        "STANDBY",
        "No freezes yet today",
        "none yet today",
        "no freezes to split",
        "FM Flight Recorder",
        "Theme · Dark",
    ] {
        assert!(t.contains(w), "/: {w}");
    }
    assert!(!t.contains("0.00\u{a0}s"), "no divide-by-zero values");
}

#[test]
fn pages_mark_their_live_parts_and_work_without_js() {
    let (app, base, _d) = server("live-parts");
    let mut o = opts();
    o.end_ms = fr_server::tz::now_ms();
    let sessions = fake::generate(&o, app.zone);
    fake::post(&sessions, &base, &app.token(), o.end_ms).unwrap();
    // Each part that app.js swaps has an id and data-live, on the page
    // that the browser gets without JS.
    let want: [(&str, &[&str]); 4] = [
        ("/", &["ov-flags", "ov-tiles", "ov-worst", "ov-latest", "srv"]),
        ("/freezes", &["fz-list", "srv"]),
        ("/clients", &["cl-tiles", "cl-table", "srv"]),
        ("/setup", &["srv"]),
    ];
    for (path, ids) in want {
        let r = get(&base, path);
        assert_eq!(r.status, 200, "{path}");
        for id in ids {
            assert!(
                r.body.contains(&format!("id=\"{id}\" data-live")),
                "{path}: no live part {id}"
            );
        }
    }
    // The lanes and the chart are live parts too.
    let page = get(&base, "/").body;
    assert!(page.contains("\" data-live aria-labelledby=") || page.contains("id=\"chart-card\" data-live"));
    // The page script opens the stream.
    let js = get(&base, "/static/app.js").body;
    assert!(js.contains("new EventSource(\"/api/v1/live\")"));
}
