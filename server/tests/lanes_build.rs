//! The lanes figure, built with a fixed "now" so the result does not depend
//! on the time of day the suite runs: binning when zoomed in, the lanes
//! past MAX_LANES, and the pressed preset.

mod common;

use common::*;
use fr_server::app::App;
use fr_server::config::Config;
use fr_server::lanes::{self, Lanes, Mode, Zoom, MAX_LANES};
use fr_server::query::{Fact, Kind, Range};
use fr_server::views::Ctx;

/// A minute of the day of 2026-11-11 in the test zone, in ms.
fn at(app: &App, min: f64) -> i64 {
    let d = chrono::NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
    app.zone.day_start(d) + (min * 60_000.0) as i64
}

fn freeze(id: i64, ts: i64, client: i64, ms: u64) -> Fact {
    Fact {
        kind: Kind::Freeze,
        id,
        ts_ms: ts,
        day: chrono::NaiveDate::from_ymd_opt(2026, 11, 11).unwrap(),
        file: "Orders".into(),
        layout: "Orders | List".into(),
        table: "Orders".into(),
        from: String::new(),
        script: String::new(),
        account: "jl".into(),
        user: String::new(),
        client_id: client,
        n: 1,
        total: ms,
        wait: ms / 2,
        max: ms,
        rt_sum: 0,
        rt_max: 0,
        srtt: 0,
        hist: None,
    }
}

/// Today's clients-mode lanes at 18:00, as the overview builds them.
fn build(name: &str, facts: &[Fact], zoom: Zoom, all_lanes: bool) -> Lanes {
    let dir = tmp(name);
    let app = App::new(Config::for_test(&dir, "America/Chicago")).unwrap();
    let now = at(&app, 18.0 * 60.0);
    let db = app.db.lock().unwrap();
    let ctx = Ctx::new(&app, &db, now);
    let r = Range::today(app.zone, now);
    lanes::build(
        &ctx,
        lanes::Input {
            mode: Mode::Clients,
            title: "Who froze, when",
            id: "lanes",
            r: &r,
            facts,
            sess: &[],
            all_lanes,
            day_url: &|_| String::new(),
            all_url: "/freezes?range=today".into(),
            live_qs: None,
            zoom,
            keep_qs: "range=today".into(),
        },
    )
}

/// The blocks that start inside lo..hi (minutes of the day).
fn blocks_in(l: &Lanes, lane: usize, lo: f64, hi: f64) -> Vec<&lanes::Block> {
    l.lanes[lane]
        .blocks
        .iter()
        .filter(|b| {
            let m: f64 = b.m.parse().unwrap();
            m >= lo && m < hi
        })
        .collect()
}

#[test]
fn zooming_in_on_a_busy_lane_shows_single_freezes() {
    // 2000 freezes from 08:00 to 17:00: one every 16 s. Before, the whole
    // lane was binned at the day step (about 1.5 min), so a 15 min window
    // showed about 10 bins of 3 freezes.
    let app_dir = tmp("lanes-busy-at");
    let app = App::new(Config::for_test(&app_dir, "America/Chicago")).unwrap();
    let n = 2000;
    let facts: Vec<Fact> = (0..n)
        .map(|i| {
            let m = 8.0 * 60.0 + i as f64 * (9.0 * 60.0 / n as f64);
            freeze(i + 1, at(&app, m), 1, 500)
        })
        .collect();
    let in_win = facts
        .iter()
        .filter(|f| f.ts_ms >= at(&app, 600.0) && f.ts_ms < at(&app, 615.0))
        .count();
    assert!(in_win > 50, "{in_win}");

    let l = build("lanes-busy", &facts, Zoom::Span(600.0, 615.0), false);
    let vis = blocks_in(&l, 0, 600.0, 615.0);
    assert_eq!(vis.len(), in_win, "one block per freeze in the window");
    assert!(vis.iter().all(|b| b.bin.is_empty()));
    // The margin is binned, so the lane stays within its block budget.
    let per_lane = lanes::MAX_BLOCKS_LANE;
    assert!(l.lanes[0].blocks.len() <= per_lane * 3, "{}", l.lanes[0].blocks.len());
    assert!(l.lanes[0].blocks.iter().any(|b| !b.bin.is_empty()));
    // Blocks are in time order.
    let xs: Vec<f64> = l.lanes[0].blocks.iter().map(|b| b.x_v).collect();
    assert!(xs.windows(2).all(|w| w[0] <= w[1]));

    // The day view bins the whole lane at day/per_lane, as before.
    let day = build("lanes-busy-day", &facts, Zoom::Day, false);
    assert!(day.lanes[0].blocks.len() <= per_lane + 1);
    assert!(day.lanes[0].blocks.iter().all(|b| !b.bin.is_empty()));
}

#[test]
fn a_window_with_more_freezes_than_the_budget_bins_at_the_window_step() {
    let app_dir = tmp("lanes-dense-at");
    let app = App::new(Config::for_test(&app_dir, "America/Chicago")).unwrap();
    // 1200 freezes in 10:00–10:15 (one every 0.75 s).
    let facts: Vec<Fact> = (0..1200)
        .map(|i| freeze(i + 1, at(&app, 600.0 + i as f64 * 0.0125), 1, 300))
        .collect();
    let l = build("lanes-dense", &facts, Zoom::Span(600.0, 615.0), false);
    let vis = blocks_in(&l, 0, 600.0, 615.0);
    // 15 min / 300 = 3 s bins: about 300 bins of 4 freezes.
    assert!(vis.len() > 250 && vis.len() <= 301, "{}", vis.len());
    assert!(vis.iter().any(|b| b.bin.starts_with("4 freezes")));
}

#[test]
fn freezes_in_lanes_past_the_cap_count_in_the_caption_and_table() {
    let app_dir = tmp("lanes-cap-at");
    let app = App::new(Config::for_test(&app_dir, "America/Chicago")).unwrap();
    let mut facts = vec![];
    let mut id = 0;
    // 15 clients with a freeze at 09:00; clients 1–12 have the long ones,
    // so 13–15 are past the cap. Client 15 alone froze in 10:00–10:15.
    for c in 1..=15 {
        id += 1;
        let ms = if c <= MAX_LANES as i64 { 10_000 } else { 1_000 };
        facts.push(freeze(id, at(&app, 540.0), c, ms));
    }
    id += 1;
    facts.push(freeze(id, at(&app, 605.0), 15, 200));

    let l = build("lanes-cap", &facts, Zoom::Span(600.0, 615.0), false);
    assert_eq!(l.lanes.len(), MAX_LANES);
    assert!(l.lanes.iter().all(|x| x.label != "client 15"));
    assert!(!l.empty, "{}", l.caption);
    assert!(l.caption.starts_with("1 freeze from"), "{}", l.caption);
    assert!(l.caption.contains("client 15"), "{}", l.caption);
    assert_eq!(l.table.len(), 1);
    assert_eq!(l.table[0].client, "client 15");
    // The "+ N more" line counts the hidden lanes in the window, and its
    // link shows every lane at the same range.
    assert!(l.more.starts_with("+ 3 more clients"), "{}", l.more);
    assert!(l.more.ends_with(" · 1"), "{}", l.more);
    assert!(l.more_url.contains("lanes=all"), "{}", l.more_url);
    assert!(l.more_url.contains("span=600.00-615.00"), "{}", l.more_url);
    // The navigator plots every freeze of the day, hidden lanes too: the
    // 10:05 freeze has a bar.
    let nav = l.nav.as_ref().unwrap();
    assert_eq!(nav.bars.len(), 2, "09:00 and 10:05");

    let all = build("lanes-cap-all", &facts, Zoom::Span(600.0, 615.0), true);
    assert_eq!(all.lanes.len(), 15);
    assert!(all.more.is_empty());
}

#[test]
fn a_followed_length_presses_the_preset_of_that_length_only() {
    let app_dir = tmp("lanes-press-at");
    let app = App::new(Config::for_test(&app_dir, "America/Chicago")).unwrap();
    let facts = vec![freeze(1, at(&app, 17.0 * 60.0), 1, 500)];
    let pressed = |l: &Lanes| -> Vec<String> {
        l.zoom_links
            .iter()
            .filter(|z| z.on)
            .map(|z| z.label.clone())
            .collect()
    };
    // 57 min plus the look-ahead pad is about 60.4 min: not "1 h".
    let l = build("lanes-press-57", &facts, Zoom::Last(57.0), false);
    assert!(pressed(&l).is_empty(), "{:?}", pressed(&l));
    let l = build("lanes-press-60", &facts, Zoom::Last(60.0), false);
    assert_eq!(pressed(&l), ["1 h"]);
    let l = build("lanes-press-span", &facts, Zoom::Span(600.0, 660.0), false);
    assert_eq!(pressed(&l), ["1 h"]);
}
