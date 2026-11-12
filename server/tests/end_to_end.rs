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
    assert!(t.contains("1 freeze<"), "freezes count: {t}");
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
                "Who froze, when",
                "Latest freezes",
                "connected",
                "Last event",
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
            &[
                "Filter layouts",
                "Round trips",
                "Server share",
                "data-tk",
                "<tfoot>",
            ],
        ),
        (
            "/scripts?range=7d",
            &[
                "Filter scripts",
                "Seen in",
                "Busy time",
                "No script name",
                "scripts=0",
            ],
        ),
        (
            "/users",
            &[
                "Filter users",
                "Shared clients (2+ users)",
                "Users on 2+ clients",
                "arivera",
                "A. Rivera",
            ],
        ),
        (
            "/clients",
            &[
                "Filter clients",
                "FileMaker",
                "0.0.9",
                "Online",
                "SRTT median",
            ],
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
    assert!(
        r.body.contains("<tr data-href=")
            && r.body.contains("data-id=")
            && r.body.contains("fz-table-count")
    );
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
    assert!(
        page.contains("threshold=300; port=443; send_ms=250; ping_ms=15000"),
        "{page}"
    );
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
    let want: [(&str, &[&str]); 7] = [
        ("/", &["ov-flags", "ov-tiles", "ov-latest", "srv"]),
        ("/freezes", &["fz-table", "srv"]),
        ("/layouts", &["lay-table", "srv"]),
        ("/scripts", &["scr-table", "srv"]),
        ("/clients", &["cl-table", "srv"]),
        ("/users", &["usr-table", "srv"]),
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
    assert!(
        page.contains("\" data-live aria-labelledby=")
            || page.contains("id=\"chart-card\" data-live")
    );
    // The page script opens the stream.
    let js = get(&base, "/static/app.js").body;
    assert!(js.contains("new EventSource(\"/api/v1/live\")"));
}

#[test]
fn sidebar_toggle_is_hidden_without_js_and_compact_tiles_are_styled() {
    let (_app, base, _d) = server("sidebar");
    let page = get(&base, "/").body;
    // The toggle starts hidden: without JS the sidebar stays open.
    assert!(page.contains("id=\"side-toggle\" aria-controls=\"side-nav\" aria-expanded=\"true\""));
    assert!(page.contains("title=\"Collapse sidebar ([)\" hidden>"));
    assert!(page.contains("<nav class=\"nav\" id=\"side-nav\""));
    // The state comes from localStorage before the first paint, in try/catch.
    assert!(page.contains(
        "try{t=localStorage.getItem(\"fr-theme\");s=localStorage.getItem(\"fr-side\")}catch(e){}"
    ));
    let js = get(&base, "/static/app.js").body;
    assert!(js.contains("try { localStorage.setItem(\"fr-side\""));
    let css = get(&base, "/static/app.css").body;
    assert!(css.contains(".side-rail{--side:60px}"));
    assert!(css.contains(".card.tile{padding:8px 14px 9px;gap:1px}"));
}

#[test]
fn overview_shows_the_newest_freezes_first_and_no_aggregates() {
    let (app, base, _d) = server("live-first");
    let mut o = opts();
    o.end_ms = fr_server::tz::now_ms();
    let sessions = fake::generate(&o, app.zone);
    fake::post(&sessions, &base, &app.token(), o.end_ms).unwrap();
    let page = get(&base, "/").body;
    let t = text(&page);
    // The aggregates are on the Layouts and Scripts pages only.
    assert!(!t.contains("Worst layouts") && !t.contains("Worst scripts"));
    assert!(!page.contains("id=\"ov-worst\""));
    // The timeline comes first, then "Latest freezes".
    let latest = page.find("id=\"ov-latest\"").expect("latest freezes");
    let lanes = page.find("Who froze, when").expect("lanes");
    assert!(lanes < latest, "the timeline is above latest freezes");
    let end = latest + page[latest..].find("</table>").expect("table end");
    // Rows carry their id (app.js marks new rows) and a machine time.
    assert!(page.contains("<tr class=\"fr\" data-id=\""));
    assert!(page.contains("<time datetime=\"20"));
    // Newest first: the times of the rows go down.
    let part = &page[latest..end];
    let times: Vec<&str> = part
        .split("<time datetime=\"")
        .skip(1)
        .map(|x| x.split('"').next().unwrap())
        .collect();
    assert!(times.len() > 1);
    assert!(times.windows(2).all(|w| w[0] >= w[1]), "newest first");
    // The time format: "Today HH:MM:SS"; the zone is in the footer and the
    // tooltips.
    let abbr = app.zone.abbr(fr_server::tz::now_ms());
    assert!(t.contains("Today "));
    assert!(
        page.contains(&format!(">{abbr}</span></div>")),
        "the zone in the footer"
    );
    assert!(page.contains(&format!(" {abbr}\"><time datetime=")));
}

#[test]
fn overview_is_calm() {
    let (app, base, _d) = server("calm");
    let mut o = opts();
    o.end_ms = fr_server::tz::now_ms();
    let sessions = fake::generate(&o, app.zone);
    fake::post(&sessions, &base, &app.token(), o.end_ms).unwrap();
    let page = get(&base, "/").body;
    // Latest freezes: time, duration with the split bar, layout, account;
    // 25 rows at most.
    let latest = page.find("id=\"ov-latest\"").unwrap();
    let part = &page[latest..latest + page[latest..].find("</table>").unwrap()];
    assert!(part.contains("<th>Time</th><th class=\"r\">Duration</th><th><span class=\"vh\">Server and client split</span></th><th>Layout</th><th>Account</th></tr>"));
    for gone in ["<th>From</th>", "<th>Client</th>", "Round trips"] {
        assert!(!part.contains(gone), "{gone}");
    }
    let rows = part.matches("<tr class=\"fr\"").count();
    assert!(rows > 8 && rows <= 25, "{rows}");
    // No Script column. The script follows the layout in the same cell,
    // muted and a link to the script page; no script, no suffix.
    assert!(!part.contains("<th>Script</th>"));
    let layout_cells = part.matches("<td class=\"c-l\">").count();
    assert_eq!(layout_cells, rows);
    let with = part
        .matches("<span class=\"c-sc\">(<a class=\"lite\" href=\"/script?")
        .count();
    assert!(with > 0 && with <= rows, "{with}");
    assert_eq!(part.matches("<span class=\"c-sc\">").count(), with);
    assert!(!part.contains("<td class=\"c-sc\">"));
    assert!(!part.contains("No script name"));
    // Each row has the split bar, with the seconds in its tooltip.
    assert_eq!(
        part.matches("<td class=\"c-s\"><div class=\"hb\" role=\"img\"")
            .count(),
        rows
    );
    assert!(
        part.contains("<div class=\"hs\" style=\"width: ")
            && part.contains("<div class=\"hc\" style=\"width: ")
    );
    let tip = part
        .split("class=\"hb\" role=\"img\" aria-label=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(
        tip.starts_with("Server ") && tip.contains(" · Client "),
        "{tip}"
    );
    // The Freezes page keeps all columns.
    let fz = get(&base, "/freezes").body;
    for kept in [
        "data-key=\"from\"",
        "data-key=\"client\"",
        "data-key=\"script\"",
        "Round trips",
    ] {
        assert!(fz.contains(kept), "{kept}");
    }
    // Tiles: one number and at most one short line.
    let tiles = &page[page.find("id=\"ov-tiles\"").unwrap()..];
    // The tiles part ends where the timeline (or the chart) begins; both
    // come before the Latest freezes card. A missing end is a failure, not
    // a fallback length (that reached into the lane labels).
    let end = ["<figure", "id=\"ov-latest\""]
        .iter()
        .filter_map(|m| tiles.find(m))
        .min()
        .expect("end of the tiles part");
    let tiles = &tiles[..end];
    assert!(
        tiles.contains("class=\"card tile\""),
        "the slice has the tiles"
    );
    for gone in [
        "script busy",
        "per freeze",
        "offline",
        "accounts",
        "Server 0",
        "Client 0",
    ] {
        assert!(!tiles.contains(gone), "{gone}");
    }
    // The timeline: no newest line, no scale note, no lane totals, no
    // summary sentence in view (it is in Table view).
    let fig =
        &page[page.find("<figure class=\"card lanes").unwrap()..page.find("</figure>").unwrap()];
    assert!(!fig.contains("class=\"latest-line\""));
    assert!(!fig.contains("class=\"lnote\""));
    assert!(!fig.contains("class=\"lnt\""));
    assert!(fig.contains("class=\"tvcap\""));
    assert!(
        fig.contains("frozen, "),
        "lane totals are in the lane tooltip"
    );
    // Footer: one short line; versions and the zone name are on Setup.
    let foot = &page[page.find("<footer").unwrap()..page.find("</footer>").unwrap()];
    assert!(!foot.contains("Plug-in") && !foot.contains("FM Flight Recorder server"));
    let setup = get(&base, "/setup").body;
    assert!(setup.contains("Plug-in ") && setup.contains("Times in "));
}

#[test]
fn timeline_blocks_are_not_links_and_zoom_is_to_time_scale() {
    let (app, base, _d) = server("zoom");
    let mut o = opts();
    o.end_ms = fr_server::tz::now_ms();
    let sessions = fake::generate(&o, app.zone);
    fake::post(&sessions, &base, &app.token(), o.end_ms).unwrap();
    let page = get(&base, "/").body;
    let fig =
        &page[page.find("<figure class=\"card lanes").unwrap()..page.find("</figure>").unwrap()];
    // Hover is enough: a block is a span with a title (no JS) and the data
    // for the tooltip, not a link to the Freeze page.
    assert!(fig.contains("<span class=\"fz"));
    assert!(!fig.contains("<a class=\"fz"));
    let blocks = fig.split("class=\"lzf\"").next().unwrap();
    assert!(
        !blocks.contains("href=\"/freeze?id="),
        "no block links to a freeze"
    );
    for a in [
        "data-t=",
        "data-layout=",
        "data-script=",
        "data-acct=",
        "data-srv=",
        "data-cli=",
        "data-pct=",
    ] {
        assert!(fig.contains(a), "{a}");
    }
    // No label on the track that can hide a block.
    assert!(!fig.contains("class=\"hl"));
    // The newest freeze is marked (the red ring).
    assert!(fig.contains(" newest\""));
    // Zoom buttons are links that keep the page query.
    for z in ["zoom=15m", "zoom=1h", "zoom=4h"] {
        assert!(fig.contains(z), "{z}");
    }
    assert!(fig.contains(">Day</a>"));
    // Day view: short freezes are drawn wider; Reset is there but hidden
    // (the header does not change its size with the range).
    assert!(!fig.contains("lanes scaled"));
    assert!(fig.contains("class=\"btn zreset off\""));
    assert!(fig.contains("short freezes drawn wider"));
    // The navigator: the day window with a mini plot and the range window.
    assert!(fig.contains("data-nav"));
    assert!(fig.contains("<rect "));
    assert!(fig.contains("class=\"nwin\""));
    assert!(fig.contains("role=\"slider\""));
    // Blocks and sessions carry their minutes, for the layout in app.js.
    assert!(fig.contains("data-m=\"") && fig.contains("data-dm=\"") && fig.contains("data-gw=\""));
    assert!(fig.contains("data-a=\""));
    // No drag-to-zoom hint on the lanes.
    assert!(!fig.contains("Drag across the lanes"));
    // Last 15 min: to time scale, with a reset link and the window label.
    let p15 = get(&base, "/?range=today&zoom=15m").body;
    let f15 = &p15[p15.find("<figure class=\"card lanes").unwrap()..p15.find("</figure>").unwrap()];
    assert!(f15.contains(" scaled\""));
    assert!(f15.contains("class=\"btn zreset\""));
    assert!(f15.contains("aria-pressed=\"true\" data-zoom data-preset=\"15 min\">15 min"));
    // It ends at now: the label says that it follows now.
    assert!(f15.contains("<span class=\"zfol\"> · follows now"));
    assert!(f15.contains("<span class=\"zspan\">15 min</span>"));
    assert!(f15.contains("Positions and widths to time scale"));
    // The live fragment keeps the zoom.
    let hx = f15
        .split("hx-get=\"/fragment/lanes?")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(hx.contains("zoom=15m"), "{hx}");
    // A span from a drag: 10:00 to 10:30, with ticks every few minutes.
    let ps = get(&base, "/?range=7d&span=600-630").body;
    assert!(ps.contains("10:00–10:30"));
    assert!(ps.contains(">10:05</span>") || ps.contains(">10:10</span>"));
    // A range that ends at now, from the navigator: zoom=37.5m.
    let pl = get(&base, "/?range=today&zoom=37.5m").body;
    assert!(pl.contains("data-follow=\"37.5\""));
    assert!(
        pl.contains("<span class=\"zspan\">37 min 30 s</span>")
            || pl.contains("<span class=\"zspan\">38 min</span>")
    );
    // A range away from now: the presets keep its center (no jump to now).
    let now_m = {
        let p = get(&base, "/?range=today").body;
        let i = p.find("data-nowm=\"").unwrap() + 11;
        p[i..].split('"').next().unwrap().parse::<f64>().unwrap()
    };
    let (a, b) = if now_m >= 720.0 {
        (60.0, 120.0)
    } else {
        (1300.0, 1360.0)
    };
    let pa = get(&base, &format!("/?range=today&span={a}-{b}")).body;
    let c = (a + b) / 2.0;
    assert!(
        pa.contains(&format!("span={:.2}-{:.2}", c - 7.5, c + 7.5)),
        "15 min keeps the center"
    );
    assert!(
        pa.contains(&format!("span={:.2}-{:.2}", c - 30.0, c + 30.0)),
        "1 h keeps the center"
    );
    assert!(pa.contains("<span class=\"zfol\" hidden>"));
    // A bad span is the day view.
    assert_eq!(get(&base, "/?span=x-y").status, 200);
}

#[test]
fn client_srtt_shows_median_and_p95_per_clock_hour() {
    let (app, base, _d) = server("srtt");
    let mut o = opts();
    o.end_ms = fr_server::tz::now_ms();
    let sessions = fake::generate(&o, app.zone);
    fake::post(&sessions, &base, &app.token(), o.end_ms).unwrap();
    let clients = get(&base, "/clients").body;
    let id = clients
        .split("href=\"/client?id=")
        .nth(1)
        .unwrap()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .to_string();
    let page = get(&base, &format!("/client?id={id}&range=7d")).body;
    let t = text(&page);
    assert!(t.contains("SRTT by hour"));
    assert!(t.contains("network round-trip time"));
    assert!(t.contains("Median") && t.contains("p95"));
    // A value in ms, on a fixed scale, per clock hour (with the day when it
    // is not today, so two days do not mix).
    assert!(page.contains("class=\"rb\""));
    assert!(page.contains("\u{a0}ms</td>"));
}

/// The first-column `data-v` of each body row of the table kit, in page order.
/// The first cell of each row in view (rows the filter hides are left out).
fn first_cells(body: &str) -> Vec<String> {
    all_rows(body)
        .into_iter()
        .filter(|r| !r[..r.find('>').unwrap()].ends_with(" hidden"))
        .map(|r| {
            r.split("data-v=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .to_string()
        })
        .collect()
}

/// Every row of the first table body, hidden or not.
fn all_rows(body: &str) -> Vec<&str> {
    let tbody = &body[body.find("<tbody").unwrap()..];
    let tbody = &tbody[..tbody.find("</tbody>").unwrap()];
    tbody.split("<tr").skip(1).collect()
}

#[test]
fn table_pages_keep_sort_and_filter_in_the_url_and_read_without_js() {
    let (app, base, _d) = server("table-state");
    let mut o = opts();
    o.end_ms = fr_server::tz::now_ms();
    let sessions = fake::generate(&o, app.zone);
    fake::post(&sessions, &base, &app.token(), o.end_ms).unwrap();

    // Default order: the most frozen time first; the rows are links and the
    // header is a set of sort links, so the page reads without JS.
    let def = get(&base, "/scripts?range=7d").body;
    assert!(def.contains("id=\"scr-table\" data-live data-tk"));
    assert!(def.contains("aria-sort=\"descending\""));
    assert!(def.contains("data-href=\"/script?"));
    let n_all = first_cells(&def).len();
    assert!(n_all > 3);

    // sort=name&dir=asc: names in order (case does not matter).
    let by_name = get(&base, "/scripts?range=7d&sort=name&dir=asc").body;
    let names = first_cells(&by_name);
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert!(by_name.contains("aria-sort=\"ascending\""));
    // The header link of that column flips the direction and keeps range.
    assert!(by_name.contains("href=\"/scripts?range=7d&#38;sort=name&#38;dir=desc\""));
    let desc = first_cells(&get(&base, "/scripts?range=7d&sort=name&dir=desc").body);
    let mut rev = sorted.clone();
    rev.reverse();
    assert_eq!(desc, rev);

    // A number column sorts by number, high to low first.
    let by_n = get(&base, "/scripts?range=7d&sort=n").body;
    assert!(by_n.contains("data-sort=\"n\" data-dir=\"desc\""));

    // The filter: rows that match, the count, the box value, and the
    // other parameters kept in the sort links and the form.
    let first = names[0].clone();
    let q = first.replace(' ', "%20").replace(':', "%3A");
    let f = get(&base, &format!("/scripts?range=7d&sort=name&dir=asc&q={q}")).body;
    let rows = first_cells(&f);
    assert!(rows.contains(&first) && rows.len() <= n_all);
    assert!(f.contains(&format!("value=\"{first}\"")));
    assert!(f.contains("name=\"range\" value=\"7d\""));
    let none = get(&base, "/scripts?range=7d&q=zzzzqqq").body;
    assert!(none.contains("0 of ") && none.contains("match"));
    assert!(first_cells(&none).is_empty());
    // The rows the filter hides stay in the page (hidden), so JS can show
    // them again when the filter is cleared; the empty line shows.
    assert_eq!(all_rows(&none).len(), n_all);
    assert!(none.contains("<p class=\"tkempty empty\" data-tkempty>"));
    assert_eq!(all_rows(&f).len(), n_all);
    assert!(def.contains("<p class=\"tkempty empty\" hidden data-tkempty>"));

    // An unknown sort column falls back to the default order.
    let bad = get(&base, "/scripts?range=7d&sort=nope&dir=asc").body;
    assert_eq!(first_cells(&bad), first_cells(&def));

    // Layouts: the same kit.
    let l = get(&base, "/layouts?range=7d&sort=name&dir=asc").body;
    let ln = first_cells(&l);
    let mut lsorted = ln.clone();
    lsorted.sort();
    assert_eq!(ln, lsorted);
    assert!(l.contains("id=\"lay-table\" data-live data-tk"));

    // Freezes: the default is the newest 100; a sort or a filter reads all
    // rows, so the first row is the longest freeze and nothing is "more".
    let newest = get(&base, "/freezes?range=7d").body;
    assert_eq!(first_cells(&newest).len(), 100);
    assert!(newest.contains("Load more"));
    let longest = get(&base, "/freezes?range=7d&sort=dur&dir=desc").body;
    let rows = first_cells(&longest);
    assert!(rows.len() > 100);
    assert!(!longest.contains("Load more"));
    let j: serde_json::Value =
        serde_json::from_str(&get(&base, "/api/v1/freezes?range=7d").body).unwrap();
    assert!(j["total"].as_u64().unwrap() as usize >= rows.len());
    let account = get(&base, "/freezes?range=7d&q=arivera").body;
    assert!(first_cells(&account).len() < rows.len());
    assert!(account.contains("arivera"));
}

#[test]
fn users_and_clients_are_tables_and_the_timeline_is_on_the_client_page() {
    let (app, base, _d) = server("users-clients");
    let mut o = opts();
    o.end_ms = fr_server::tz::now_ms();
    let sessions = fake::generate(&o, app.zone);
    fake::post(&sessions, &base, &app.token(), o.end_ms).unwrap();

    // The lists: a kit table, no stat cards, no timeline.
    for path in ["/users?range=7d", "/clients?range=7d"] {
        let b = get(&base, path).body;
        assert!(b.contains("data-tk"), "{path}");
        assert!(!b.contains("class=\"card tile\""), "{path}: stat cards");
        assert!(!b.contains("data-lanes"), "{path}: timeline");
        assert!(b.contains("data-href=\""), "{path}: row link");
    }
    let users = get(&base, "/users?range=7d").body;
    for col in [
        "Freezes",
        "Frozen time",
        "Server / client",
        "p50",
        "p95",
        "Server share",
        "Clients",
        "Main layout",
        "Last seen",
    ] {
        assert!(users.contains(col), "users column {col}");
    }
    let clients = get(&base, "/clients?range=7d").body;
    for col in [
        "Status",
        "Users",
        "Frozen time",
        "p95",
        "SRTT median",
        "Plug-in",
        "Last seen",
    ] {
        assert!(clients.contains(col), "clients column {col}");
    }
    assert!(clients.contains("Online") || clients.contains("Offline"));

    // Sort state in the URL works on both.
    let by_name = first_cells(&get(&base, "/users?range=7d&sort=account&dir=asc").body);
    let mut sorted = by_name.clone();
    sorted.sort();
    assert_eq!(by_name, sorted);
    let q = get(&base, "/clients?range=7d&q=zzzzqqq").body;
    assert!(first_cells(&q).is_empty() && q.contains("match"));

    // The client page: the timeline first, then that client's freezes, then
    // SRTT. No stat cards.
    let id = clients
        .split("href=\"/client?id=")
        .nth(1)
        .unwrap()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .to_string();
    let page = get(&base, &format!("/client?id={id}&range=7d")).body;
    assert!(!page.contains("class=\"card tile\""));
    let lanes = page.find("data-lanes").expect("timeline");
    let table = page.find("id=\"cl-events\"").expect("freezes table");
    let srtt = page.find("SRTT by hour").expect("srtt");
    assert!(lanes < table && table < srtt);
    assert!(page.contains("data-nav"), "range navigator");

    // The user page: no stat cards, the events as a table.
    let user = get(&base, "/user?account=arivera&range=7d").body;
    assert!(!user.contains("class=\"card tile\""));
    assert!(user.contains("id=\"usr-events\""));
    // Layout, script and freeze pages too.
    let any = |path: &str| {
        let list = get(&base, path).body;
        let l = list
            .split("href=\"/")
            .skip(1)
            .map(|p| p.split('"').next().unwrap())
            .find(|l| {
                l.starts_with(&format!(
                    "{}?",
                    path[1..].split('?').next().unwrap().trim_end_matches('s')
                ))
            })
            .unwrap()
            .to_string();
        get(&base, &format!("/{l}")).body
    };
    for list in ["/layouts?range=7d", "/scripts?range=7d"] {
        let detail = any(list);
        assert!(!detail.contains("class=\"card tile\""), "{list} detail");
        assert!(detail.contains("<th>Metric</th>"));
    }
}

#[test]
fn script_user_and_client_timelines_link_to_their_freezes() {
    let (app, base, _d) = server("all-freezes");
    let mut o = opts();
    o.end_ms = fr_server::tz::now_ms();
    let sessions = fake::generate(&o, app.zone);
    fake::post(&sessions, &base, &app.token(), o.end_ms).unwrap();
    // The first row of each list opens a detail page; its timeline ends
    // with "All freezes" filtered to that script, user or client.
    for (list, key) in [
        ("/scripts?range=7d", "&script="),
        ("/users?range=7d", "&account="),
        ("/clients?range=7d", "&client="),
    ] {
        let l = text(&get(&base, list).body);
        let href = l
            .split("data-href=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();
        let sep = if href.contains('?') { '&' } else { '?' };
        let page = text(&get(&base, &format!("{href}{sep}range=7d")).body);
        let fig = &page[page.find("<figure class=\"card lanes").expect(list)..];
        let fig = &fig[..fig.find("</figure>").unwrap()];
        let link = fig
            .split("<a href=\"/freezes?")
            .nth(1)
            .unwrap_or_else(|| panic!("{list}: no All freezes link"))
            .split('"')
            .next()
            .unwrap();
        assert!(link.contains(key), "{list}: {link}");
        assert!(fig.contains(">All freezes</a>"), "{list}");
    }
    // The overview has the link in its Latest freezes card, not twice.
    let ov = get(&base, "/?range=7d").body;
    let fig = &ov[ov.find("<figure class=\"card lanes").unwrap()..];
    let fig = &fig[..fig.find("</figure>").unwrap()];
    assert!(!fig.contains(">All freezes</a>"));
}

#[test]
fn the_overview_flag_bar_shows_when_a_flag_fires() {
    let (app, base, _d) = server("flagbar");
    let mut o = opts();
    o.end_ms = fr_server::tz::now_ms();
    let sessions = fake::generate(&o, app.zone);
    fake::post(&sessions, &base, &app.token(), o.end_ms).unwrap();
    // Low thresholds: every layout's p95 is over 0.1 s and every plug-in is
    // older than 99.0.0.
    let form = "flag_p95_s=0.1&flag_quiet_min=45&flag_plugin_min=99.0.0&rec_threshold_ms=300&rec_port=443&event_days=14&total_days=365";
    let r = request(
        &base,
        "POST",
        "/setup",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        form.as_bytes(),
    );
    assert_eq!(r.status, 303);
    let page = get(&base, "/?range=7d").body;
    let bar = &page[page
        .find("<div class=\"card flagbar\" id=\"ov-flags\" data-live>")
        .expect("the flag bar")..];
    let bar = &bar[..bar.find("</div>").unwrap()];
    assert!(bar.contains("<h2 class=\"ct\">Flags</h2><span class=\"flag\">"));
    assert!(bar.contains("<span class=\"fgrp\" title=\""), "{bar}");
    assert!(bar.contains("<span class=\"flag\">plug-in "), "{bar}");
    assert!(bar.contains("<a class=\"thr\" href=\"/setup#s-flags\">Thresholds</a>"));
    assert!(!page.contains("<div id=\"ov-flags\" data-live hidden></div>"));
}
