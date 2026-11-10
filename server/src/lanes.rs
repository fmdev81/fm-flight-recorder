//! "Who froze, when": the client-lanes timeline.
//!
//! One component in three modes:
//! - clients: one lane per client, x = today's hours, with a playhead at now;
//! - days: one lane per day of the range, x = hour of the day;
//! - strip: one lane around one freeze (the Freeze page), true time scale.
//!
//! Every freeze block is a link, so the graph works with no JS. Positions are
//! percentages; block widths are pixels that grow with the duration ("not to
//! time scale"). `app.js` adds the tooltip, the arrow keys and the REC clock.

use crate::fmt::{dur, dur1, enc, num, pct};
use crate::query::{Fact, Kind, Range, RangeKind};
use crate::tz::{Zone, DAY_MS};
use crate::views::{Ctx, SessRow};
use chrono::NaiveDate;
use std::collections::{BTreeMap, HashMap};

/// Lanes shown before "+ N more clients".
pub const MAX_LANES: usize = 12;
/// Blocks per lane before the freezes are binned.
pub const MAX_BLOCKS_LANE: usize = 300;
/// Blocks per graph.
pub const MAX_BLOCKS: usize = 1500;
/// Rows of the table view.
pub const TABLE_ROWS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Clients,
    Days,
    Strip,
}

#[derive(Debug, Clone, Default)]
pub struct Tick {
    pub x: String,
    pub label: String,
    /// Shown at 800 px and wider.
    pub wide: bool,
    /// Shown below 800 px.
    pub narrow: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Seg {
    pub x: String,
    pub w: String,
    pub title: String,
}

#[derive(Debug, Clone, Default)]
pub struct Block {
    pub id: i64,
    pub url: String,
    pub x: String,
    pub x_v: f64,
    pub w: u32,
    pub sw: u32,
    pub hit: bool,
    pub aria: String,
    pub t: String,
    pub d: String,
    pub from: String,
    pub layout: String,
    pub acct: String,
    pub script: String,
    pub client: String,
    pub srv: String,
    pub cli: String,
    /// "12 freezes 10:05–10:10" for a bin, else empty.
    pub bin: String,
}

#[derive(Debug, Clone, Default)]
pub struct HitLabel {
    /// CSS `left` (or `right`) of the label.
    pub pos: String,
    pub left_side: bool,
    pub layout: String,
    pub acct: String,
    pub d: String,
}

#[derive(Debug, Clone, Default)]
pub struct Lane {
    pub label: String,
    pub url: String,
    pub total: String,
    pub n: String,
    pub off: bool,
    pub sess: Vec<Seg>,
    pub blocks: Vec<Block>,
    pub hit: Option<HitLabel>,
    /// Days mode: the now line on today's lane.
    pub now_x: String,
    /// The track after this x (percent) is not recorded yet.
    pub future_x: String,
}

#[derive(Debug, Clone, Default)]
pub struct TRow {
    pub t: String,
    pub url: String,
    pub client: String,
    pub layout: String,
    pub acct: String,
    pub d: String,
    pub pct: String,
}

#[derive(Debug, Clone, Default)]
pub struct Lanes {
    pub id: String,
    pub title: String,
    pub mode_days: bool,
    pub strip: bool,
    pub compact: bool,
    pub axis: Vec<Tick>,
    /// The playhead (clients mode, when the window holds now).
    pub now_x: String,
    /// The playhead as a fraction of the track (for CSS calc).
    pub now_f: String,
    pub rec: bool,
    pub now_ms: i64,
    pub now_label: String,
    pub win0: i64,
    pub win1: i64,
    pub lanes: Vec<Lane>,
    pub caption: String,
    pub more: String,
    pub more_url: String,
    pub table: Vec<TRow>,
    pub table_more: bool,
    pub all_url: String,
    pub note: String,
    pub live_url: String,
    pub empty: bool,
}

impl Lanes {
    pub fn has_now(&self) -> bool {
        !self.now_x.is_empty()
    }
}

/// One freeze, as the lanes need it.
#[derive(Debug, Clone)]
struct Ev {
    id: i64,
    ts: i64,
    d: u64,
    wait: u64,
    client: i64,
    layout: String,
    from: String,
    acct: String,
    user: String,
    script: String,
}

fn events(f: &[Fact]) -> Vec<Ev> {
    let mut v: Vec<Ev> = f
        .iter()
        .filter(|x| x.kind == Kind::Freeze && x.id > 0)
        .map(|x| Ev {
            id: x.id,
            ts: x.ts_ms,
            d: x.total,
            wait: x.wait.min(x.total),
            client: x.client_id,
            layout: x.layout.clone(),
            from: x.from.clone(),
            acct: x.account.clone(),
            user: x.user.clone(),
            script: x.script.clone(),
        })
        .collect();
    v.sort_by_key(|e| (e.ts, e.id));
    v
}

/// Block width in px: 0.25 s → 6, 1 s → 12, 4 s → 18, 16 s → 24, 64 s+ → 30–40.
pub fn glyph_px(ms: u64) -> u32 {
    let s = (ms as f64 / 1000.0).max(0.001);
    (6.0 + 3.0 * (s / 0.25).log2()).round().clamp(6.0, 40.0) as u32
}

/// Width of the server part of a block of `w` px.
pub fn server_px(w: u32, share: f64) -> u32 {
    let inner = w.saturating_sub(2) as f64;
    ((inner * share).round() as u32).clamp(2, w.saturating_sub(2).max(2))
}

/// Minutes since local midnight (wall clock, so DST days line up).
fn tod(z: Zone, ms: i64) -> f64 {
    let s = z.fmt(ms, "%H %M %S");
    let mut it = s.split(' ').map(|p| p.parse::<f64>().unwrap_or(0.0));
    let (h, m, sec) = (
        it.next().unwrap_or(0.0),
        it.next().unwrap_or(0.0),
        it.next().unwrap_or(0.0),
    );
    h * 60.0 + m + sec / 60.0 + (ms.rem_euclid(1000)) as f64 / 60_000.0
}

fn pc(v: f64) -> String {
    format!("{:.3}%", v.clamp(0.0, 100.0))
}

fn acct_label(e: &Ev) -> String {
    if e.acct.is_empty() {
        "—".into()
    } else {
        e.acct.clone()
    }
}

fn script_label(s: &str) -> String {
    if s.is_empty() {
        "No script name".into()
    } else {
        s.into()
    }
}

/// The page context the lanes need.
pub struct Input<'a> {
    pub mode: Mode,
    pub title: &'a str,
    pub id: &'a str,
    pub r: &'a Range,
    pub facts: &'a [Fact],
    /// Sessions for the lane segments (empty: no segments).
    pub sess: &'a [SessRow],
    /// Clients mode: show every lane (`?lanes=all`).
    pub all_lanes: bool,
    /// Link of a day lane (days mode).
    pub day_url: &'a dyn Fn(NaiveDate) -> String,
    /// "All freezes" with the same filters.
    pub all_url: String,
    /// The query string for the live fragment (clients mode, today).
    pub live_qs: Option<String>,
}

struct Win {
    lo: f64,
    hi: f64,
}

impl Win {
    fn x(&self, m: f64) -> f64 {
        (m - self.lo) / (self.hi - self.lo).max(1.0) * 100.0
    }
}

/// A lane before layout: its events and segments in window minutes.
struct Raw {
    label: String,
    url: String,
    evs: Vec<Ev>,
    /// (start min, end min, title)
    sess: Vec<(f64, f64, String)>,
    off: bool,
    day: Option<NaiveDate>,
    sort_total: u64,
}

pub fn build(ctx: &Ctx, inp: Input) -> Lanes {
    let z = ctx.z;
    let now = ctx.now;
    let evs = events(inp.facts);
    let mut out = Lanes {
        id: inp.id.into(),
        title: inp.title.into(),
        mode_days: inp.mode == Mode::Days,
        all_url: inp.all_url.clone(),
        now_ms: now,
        now_label: z.hms(now),
        ..Default::default()
    };
    let today = z.date_of(now);
    let now_m = tod(z, now);
    let mut raws: Vec<Raw> = vec![];
    match inp.mode {
        Mode::Clients => {
            let mut by_client: HashMap<i64, Vec<Ev>> = HashMap::new();
            for e in &evs {
                by_client.entry(e.client).or_default().push(e.clone());
            }
            let mut seen: Vec<i64> = ctx
                .clients
                .iter()
                .filter(|c| c.last_seen_ms >= now - 30 * DAY_MS || by_client.contains_key(&c.id))
                .map(|c| c.id)
                .collect();
            for id in by_client.keys() {
                if !seen.contains(id) {
                    seen.push(*id);
                }
            }
            for id in seen {
                let mine: Vec<&SessRow> = inp.sess.iter().filter(|s| s.client == id).collect();
                let e = by_client.remove(&id).unwrap_or_default();
                let sess = mine
                    .iter()
                    .map(|s| {
                        let end = if s.open { now } else { s.end };
                        let a = s.start.max(inp.r.start_ms);
                        let b = end.min(inp.r.end_ms).min(now);
                        (
                            tod(z, a),
                            if z.date_of(b) > z.date_of(a) {
                                24.0 * 60.0
                            } else {
                                tod(z, b)
                            },
                            format!(
                                "Session {}–{}",
                                z.fmt(s.start, "%H:%M"),
                                if s.open {
                                    "open".to_string()
                                } else {
                                    z.fmt(s.end, "%H:%M")
                                }
                            ),
                        )
                    })
                    .collect();
                raws.push(Raw {
                    label: ctx.client_name(id),
                    url: format!("/client?id={id}&range=today"),
                    sort_total: e.iter().map(|x| x.d).sum(),
                    off: !mine.is_empty() && !mine.iter().any(|s| s.open),
                    evs: e,
                    sess,
                    day: None,
                });
            }
            out.rec = inp.sess.iter().any(|s| s.open);
            raws.sort_by(|a, b| b.sort_total.cmp(&a.sort_total).then(a.label.cmp(&b.label)));
            if raws.len() > MAX_LANES && !inp.all_lanes {
                raws.retain(|l| !l.evs.is_empty() || !l.sess.is_empty());
                if raws.len() > MAX_LANES {
                    let rest = raws.split_off(MAX_LANES);
                    let t: u64 = rest.iter().map(|l| l.sort_total).sum();
                    let n: usize = rest.iter().map(|l| l.evs.len()).sum();
                    out.more = format!(
                        "+ {} more clients · {} · {}",
                        rest.len(),
                        dur(t),
                        num(n as u64)
                    );
                    out.more_url = "/clients?lanes=all".into();
                }
            }
        }
        Mode::Days => {
            let mut by_day: BTreeMap<NaiveDate, Vec<Ev>> = BTreeMap::new();
            for e in &evs {
                by_day.entry(z.date_of(e.ts)).or_default().push(e.clone());
            }
            let first_raw = inp.r.from.max(ctx.cutoff);
            if inp.r.from < ctx.cutoff {
                out.note = "Older days have daily totals only.".into();
            }
            for d in inp.r.days().into_iter().rev() {
                if d < first_raw || d > today {
                    continue;
                }
                let (ds, de) = (z.day_start(d), z.day_end(d));
                let sess = inp
                    .sess
                    .iter()
                    .filter(|s| s.start < de && (if s.open { now } else { s.end }) >= ds)
                    .map(|s| {
                        let end = if s.open { now } else { s.end };
                        let a = s.start.max(ds);
                        let b = end.min(de - 1);
                        (
                            tod(z, a),
                            tod(z, b),
                            format!(
                                "Session {}–{}",
                                z.fmt(s.start, "%H:%M"),
                                if s.open {
                                    "open".to_string()
                                } else {
                                    z.fmt(s.end, "%H:%M")
                                }
                            ),
                        )
                    })
                    .collect();
                let e = by_day.remove(&d).unwrap_or_default();
                raws.push(Raw {
                    label: d.format("%a %-d %b").to_string(),
                    url: (inp.day_url)(d),
                    sort_total: e.iter().map(|x| x.d).sum(),
                    evs: e,
                    sess,
                    off: false,
                    day: Some(d),
                });
            }
            out.compact = raws.len() > 14;
        }
        Mode::Strip => {}
    }

    // The window, in minutes of the day.
    let mut lo = 8.0 * 60.0;
    let mut hi = 18.0 * 60.0;
    let has_today = inp.r.from <= today && today <= inp.r.to;
    for l in &raws {
        for e in &l.evs {
            let m = tod(z, e.ts);
            lo = f64::min(lo, m);
            hi = f64::max(hi, m + e.d as f64 / 60_000.0);
        }
        for (a, b, _) in &l.sess {
            lo = f64::min(lo, *a);
            hi = f64::max(hi, *b);
        }
    }
    if has_today {
        lo = f64::min(lo, now_m - 60.0);
        hi = f64::max(hi, now_m + 30.0);
    }
    let lo = ((lo / 60.0).floor() * 60.0).max(0.0);
    let hi = ((hi / 60.0).ceil() * 60.0).min(24.0 * 60.0);
    let win = Win { lo, hi };
    out.win0 = lo as i64;
    out.win1 = hi as i64;
    let playhead = inp.mode == Mode::Clients && has_today;
    if playhead {
        out.now_x = pc(win.x(now_m));
        out.now_f = format!("{:.5}", win.x(now_m).clamp(0.0, 100.0) / 100.0);
    }
    let mut h = (lo / 60.0) as i64;
    while (h as f64) * 60.0 <= hi {
        let m = h as f64 * 60.0;
        let near_now = playhead && (m - now_m).abs() < 25.0;
        if !near_now && (h % 2 == 0 || h % 3 == 0) {
            out.axis.push(Tick {
                x: pc(win.x(m)),
                label: format!("{h:02}:00"),
                wide: h % 2 == 0,
                narrow: h % 3 == 0,
            });
        }
        h += 1;
    }

    // The longest freeze of the graph gets the ring and the label.
    let longest = raws
        .iter()
        .flat_map(|l| l.evs.iter())
        .max_by_key(|e| (e.d, std::cmp::Reverse(e.ts)))
        .map(|e| e.id);
    let total_ev: usize = raws.iter().map(|l| l.evs.len()).sum();
    let per_lane = MAX_BLOCKS_LANE.min((MAX_BLOCKS / raws.len().max(1)).max(30));
    for raw in &raws {
        let mut lane = Lane {
            label: raw.label.clone(),
            url: raw.url.clone(),
            total: if raw.evs.is_empty() {
                "—".into()
            } else {
                dur(raw.sort_total)
            },
            n: num(raw.evs.len() as u64),
            off: raw.off,
            ..Default::default()
        };
        lane.sess = raw
            .sess
            .iter()
            .filter(|(a, b, _)| b > a)
            .map(|(a, b, t)| {
                let x0 = win.x(*a);
                Seg {
                    x: pc(x0),
                    w: pc((win.x(*b) - x0).max(0.2)),
                    title: t.clone(),
                }
            })
            .collect();
        if let Some(d) = raw.day {
            if d == today {
                lane.now_x = pc(win.x(now_m));
            }
        }
        if inp.mode == Mode::Clients && has_today {
            lane.future_x = out.now_x.clone();
        }
        let day_prefix = |e: &Ev| -> String {
            if inp.mode == Mode::Days {
                format!("{} ", z.fmt(e.ts, "%a %-d %b"))
            } else {
                String::new()
            }
        };
        if raw.evs.len() <= per_lane {
            for e in &raw.evs {
                let m = tod(z, e.ts);
                let w = glyph_px(e.d);
                let share = if e.d > 0 {
                    e.wait as f64 / e.d as f64
                } else {
                    0.0
                };
                let client = ctx.client_name(e.client);
                let when = format!("{}{}", day_prefix(e), z.hms(e.ts));
                let mut acct = acct_label(e);
                if !e.user.is_empty() && e.user != e.acct {
                    acct = format!("{acct} {}", e.user);
                }
                lane.blocks.push(Block {
                    id: e.id,
                    url: format!("/freeze?id={}", e.id),
                    x_v: win.x(m),
                    x: pc(win.x(m)),
                    w,
                    sw: server_px(w, share),
                    hit: Some(e.id) == longest,
                    aria: format!(
                        "{when}, {client}, {}, {}, {} server wait",
                        e.layout,
                        dur1(e.d),
                        pct(share)
                    ),
                    t: when,
                    d: dur1(e.d),
                    from: e.from.clone(),
                    layout: e.layout.clone(),
                    acct,
                    script: script_label(&e.script),
                    client,
                    srv: dur1(e.wait),
                    cli: dur1(e.d - e.wait),
                    bin: String::new(),
                });
            }
        } else {
            // Bin the freezes into windows of window/per_lane minutes.
            let step = (hi - lo) / per_lane as f64;
            let mut bins: BTreeMap<i64, Vec<&Ev>> = BTreeMap::new();
            for e in &raw.evs {
                let k = ((tod(z, e.ts) - lo) / step).floor() as i64;
                bins.entry(k).or_default().push(e);
            }
            for (k, v) in bins {
                let top = v.iter().max_by_key(|e| e.d).copied().unwrap();
                let sum: u64 = v.iter().map(|e| e.d).sum();
                let wsum: u64 = v.iter().map(|e| e.wait).sum();
                let share = if sum > 0 {
                    wsum as f64 / sum as f64
                } else {
                    0.0
                };
                let m0 = lo + k as f64 * step;
                let m1 = m0 + step;
                let hm = |m: f64| format!("{:02}:{:02}", (m / 60.0) as i64, (m % 60.0) as i64);
                let label = if v.len() == 1 {
                    String::new()
                } else {
                    format!("{} freezes {}–{}", v.len(), hm(m0), hm(m1))
                };
                let w = glyph_px(top.d);
                let client = ctx.client_name(top.client);
                let when = format!("{}{}", day_prefix(top), z.hms(top.ts));
                lane.blocks.push(Block {
                    id: top.id,
                    url: format!("/freeze?id={}", top.id),
                    x_v: win.x(tod(z, top.ts)),
                    x: pc(win.x(tod(z, top.ts))),
                    w,
                    sw: server_px(w, share),
                    hit: v.iter().any(|e| Some(e.id) == longest),
                    aria: if label.is_empty() {
                        format!(
                            "{when}, {client}, {}, {}, {} server wait",
                            top.layout,
                            dur1(top.d),
                            pct(share)
                        )
                    } else {
                        format!(
                            "{}{label} · longest {} · total {}",
                            day_prefix(top),
                            dur1(top.d),
                            dur(sum)
                        )
                    },
                    t: when,
                    d: dur1(top.d),
                    from: top.from.clone(),
                    layout: top.layout.clone(),
                    acct: acct_label(top),
                    script: script_label(&top.script),
                    client,
                    srv: dur1(wsum),
                    cli: dur1(sum - wsum),
                    bin: if label.is_empty() {
                        String::new()
                    } else {
                        format!("{label} · total {}", dur(sum))
                    },
                });
            }
        }
        if let Some(b) = lane.blocks.iter().find(|b| b.hit) {
            let left_side = b.x_v > 62.0;
            lane.hit = Some(HitLabel {
                pos: if left_side {
                    format!("calc({:.3}% - 8px)", b.x_v)
                } else {
                    format!("calc({} + {}px)", b.x, b.w + 8)
                },
                left_side,
                layout: b.layout.clone(),
                acct: b.acct.split(' ').next().unwrap_or("").to_string(),
                d: b.d.clone(),
            });
        }
        out.lanes.push(lane);
    }

    // Caption and table view.
    let all_evs: Vec<&Ev> = raws.iter().flat_map(|l| l.evs.iter()).collect();
    out.empty = all_evs.is_empty();
    let top = all_evs
        .iter()
        .max_by_key(|e| (e.d, std::cmp::Reverse(e.ts)))
        .copied();
    match inp.mode {
        Mode::Clients => {
            if let Some(t) = top {
                let nclients = {
                    let mut c: Vec<i64> = all_evs.iter().map(|e| e.client).collect();
                    c.sort_unstable();
                    c.dedup();
                    c.len()
                };
                let first = inp
                    .sess
                    .iter()
                    .map(|s| s.start.max(inp.r.start_ms))
                    .chain(all_evs.iter().map(|e| e.ts))
                    .min()
                    .unwrap_or(t.ts);
                out.caption = format!(
                    "{} {} on {} {} since {}. Longest {} at {} on {} ({}, {}).",
                    num(total_ev as u64),
                    if total_ev == 1 { "freeze" } else { "freezes" },
                    nclients,
                    if nclients == 1 { "client" } else { "clients" },
                    z.fmt(first, "%H:%M"),
                    dur1(t.d),
                    z.hms(t.ts),
                    ctx.client_name(t.client),
                    t.layout,
                    acct_label(t)
                );
            } else {
                let connected = ctx.clients.iter().filter(|c| c.connected).count();
                let last: Option<i64> = ctx
                    .db
                    .query_row("SELECT max(ts_ms) FROM events", [], |r| r.get(0))
                    .ok()
                    .flatten();
                out.caption = format!(
                    "No freezes yet today. {connected} of {} clients connected; {}.",
                    ctx.clients.len(),
                    match last {
                        Some(t) => format!("last event {}", z.fmt(t, "%a %-d %b %H:%M:%S")),
                        None => "no events yet".into(),
                    }
                );
            }
        }
        Mode::Days => {
            if let Some(t) = top {
                let ndays = raws.iter().filter(|l| !l.evs.is_empty()).count();
                out.caption = format!(
                    "{} {} on {} {}. Longest {} on {} at {} on {} ({}, {}).",
                    num(total_ev as u64),
                    if total_ev == 1 { "freeze" } else { "freezes" },
                    ndays,
                    if ndays == 1 { "day" } else { "days" },
                    dur1(t.d),
                    z.fmt(t.ts, "%a %-d %b"),
                    z.hms(t.ts),
                    ctx.client_name(t.client),
                    t.layout,
                    acct_label(t)
                );
            } else if inp.r.kind == RangeKind::Today {
                out.caption = "No freezes yet today.".into();
            } else {
                out.caption = "No freezes in this range.".into();
            }
        }
        Mode::Strip => {}
    }
    let mut rows: Vec<&Ev> = all_evs.clone();
    rows.sort_by_key(|e| std::cmp::Reverse((e.ts, e.id)));
    out.table_more = rows.len() > TABLE_ROWS;
    out.table = rows
        .into_iter()
        .take(TABLE_ROWS)
        .map(|e| TRow {
            t: if inp.mode == Mode::Days {
                z.fmt(e.ts, "%a %-d %b %H:%M:%S")
            } else {
                z.hms(e.ts)
            },
            url: format!("/freeze?id={}", e.id),
            client: ctx.client_name(e.client),
            layout: e.layout.clone(),
            acct: acct_label(e),
            d: dur1(e.d),
            pct: if e.d > 0 {
                pct(e.wait as f64 / e.d as f64)
            } else {
                String::new()
            },
        })
        .collect();
    if let Some(qs) = inp.live_qs {
        if playhead && inp.r.kind == RangeKind::Today {
            out.live_url = format!("/fragment/lanes?{qs}");
        }
    }
    out
}

/// The Freeze page strip: one lane for the client, ±15 min around the
/// freeze, true time scale. This freeze is ringed; the others are links.
pub fn strip(ctx: &Ctx, client: i64, at: i64, id: i64, facts: &[Fact]) -> Lanes {
    let z = ctx.z;
    let (w0, w1) = (at - 15 * 60_000, at + 15 * 60_000);
    let span = (w1 - w0) as f64;
    let x = |t: i64| (t - w0) as f64 / span * 100.0;
    let mut lane = Lane {
        label: ctx.client_name(client),
        url: format!("/client?id={client}&range=today"),
        ..Default::default()
    };
    let evs: Vec<Ev> = events(facts)
        .into_iter()
        .filter(|e| e.client == client && e.ts >= w0 && e.ts < w1)
        .collect();
    for e in &evs {
        let w = glyph_px(e.d);
        let share = if e.d > 0 {
            e.wait as f64 / e.d as f64
        } else {
            0.0
        };
        lane.blocks.push(Block {
            id: e.id,
            url: format!("/freeze?id={}", e.id),
            x_v: x(e.ts),
            x: pc(x(e.ts)),
            w,
            sw: server_px(w, share),
            hit: e.id == id,
            aria: format!(
                "{}, {}, {}, {} server wait{}",
                z.hms(e.ts),
                e.layout,
                dur1(e.d),
                pct(share),
                if e.id == id { ", this freeze" } else { "" }
            ),
            t: z.hms(e.ts),
            d: dur1(e.d),
            from: e.from.clone(),
            layout: e.layout.clone(),
            acct: acct_label(e),
            script: script_label(&e.script),
            client: lane.label.clone(),
            srv: dur1(e.wait),
            cli: dur1(e.d - e.wait),
            bin: String::new(),
        });
    }
    let total: u64 = evs.iter().map(|e| e.d).sum();
    lane.total = if evs.is_empty() {
        "—".into()
    } else {
        dur(total)
    };
    lane.n = num(evs.len() as u64);
    let mut axis = vec![];
    let first = (w0 + 5 * 60_000 - 1).div_euclid(5 * 60_000) * 5 * 60_000;
    let mut t = first;
    while t <= w1 {
        axis.push(Tick {
            x: pc(x(t)),
            label: z.fmt(t, "%H:%M"),
            wide: true,
            narrow: (t / 60_000) % 10 == 0,
        });
        t += 5 * 60_000;
    }
    let n = evs.len();
    Lanes {
        id: "strip".into(),
        title: "Around this freeze".into(),
        strip: true,
        axis,
        caption: format!(
            "{} {} on {} from {} to {}.",
            num(n as u64),
            if n == 1 { "freeze" } else { "freezes" },
            lane.label,
            z.fmt(w0, "%H:%M"),
            z.fmt(w1, "%H:%M")
        ),
        lanes: vec![lane],
        empty: n == 0,
        now_ms: ctx.now,
        ..Default::default()
    }
}

/// The query string that keeps a file filter for links.
pub fn file_qs(file: &Option<String>) -> String {
    file.as_ref()
        .map(|f| format!("&file={}", enc(f)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_widths_grow_with_duration() {
        assert_eq!(glyph_px(250), 6);
        assert_eq!(glyph_px(1000), 12);
        assert_eq!(glyph_px(4000), 18);
        assert_eq!(glyph_px(16_000), 24);
        assert_eq!(glyph_px(64_000), 30);
        assert_eq!(glyph_px(10_000_000), 40);
        assert_eq!(glyph_px(100), 6);
        assert_eq!(server_px(18, 0.8), 13);
        assert_eq!(server_px(6, 0.0), 2);
        assert_eq!(server_px(6, 1.0), 4);
    }
}
