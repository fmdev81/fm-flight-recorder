//! "Who froze, when": the client-lanes timeline.
//!
//! One component in three modes:
//! - clients: one lane per client, x = today's hours, with a playhead at now;
//! - days: one lane per day of the range, x = hour of the day;
//! - strip: one lane around one freeze (the Freeze page), true time scale.
//!
//! A block is not a link: hover (or focus) shows the details. Without JS the
//! block has a `title`. Positions are percentages. In the day view, block
//! widths are pixels that grow with the duration ("not to time scale"). At a
//! close zoom (4 h or less) positions and widths are to time scale.
//! `app.js` adds the tooltip, the arrow keys, drag and pinch zoom and the
//! REC clock.
//!
//! Zoom: `zoom=15m|1h|4h` shows the last 15 min, 1 h or 4 h up to now;
//! `span=LO-HI` (minutes of the day, from a drag) shows that window. Both are
//! query parameters, so the zoom links work with no JS and a live update
//! keeps the zoom.

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
/// The window at or below which the lanes are to time scale (minutes).
pub const SCALE_MIN: f64 = 240.0;
/// The smallest zoom window (minutes).
pub const MIN_SPAN: f64 = 1.0;

/// The time window of the lanes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Zoom {
    /// The whole day (hours with activity, as before).
    #[default]
    Day,
    /// The last N minutes up to now (today only).
    Last(f64),
    /// From LO to HI, minutes of the day.
    Span(f64, f64),
}

impl Zoom {
    /// From the query: `span` wins over `zoom`.
    pub fn parse(zoom: &str, span: &str) -> Zoom {
        if let Some((a, b)) = span.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
                if a.is_finite() && b.is_finite() {
                    let (a, b) = (a.min(b).clamp(0.0, 1440.0), a.max(b).clamp(0.0, 1440.0));
                    let (a, b) = if b - a < MIN_SPAN {
                        let c = ((a + b) / 2.0).clamp(MIN_SPAN / 2.0, 1440.0 - MIN_SPAN / 2.0);
                        (c - MIN_SPAN / 2.0, c + MIN_SPAN / 2.0)
                    } else {
                        (a, b)
                    };
                    return Zoom::Span(a, b);
                }
            }
        }
        match zoom {
            "15m" => Zoom::Last(15.0),
            "1h" => Zoom::Last(60.0),
            "4h" => Zoom::Last(240.0),
            _ => Zoom::Day,
        }
    }

    fn key(&self) -> &'static str {
        match self {
            Zoom::Day => "day",
            Zoom::Last(m) if *m <= 15.0 => "15m",
            Zoom::Last(m) if *m <= 60.0 => "1h",
            Zoom::Last(_) => "4h",
            Zoom::Span(..) => "span",
        }
    }
}

/// A zoom button: a link that keeps the other query parameters.
#[derive(Debug, Clone, Default)]
pub struct ZLink {
    pub label: String,
    pub href: String,
    pub on: bool,
}

/// The newest freeze of the graph, for the header line.
#[derive(Debug, Clone, Default)]
pub struct Latest {
    pub t: String,
    pub iso: String,
    pub ago: String,
    pub layout: String,
    pub acct: String,
    pub client: String,
    pub d: String,
}

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
    /// Just after the playhead, where the REC label may cover it. Hidden
    /// until app.js measures that it is clear of the label.
    pub near: bool,
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
    pub x: String,
    pub x_v: f64,
    pub w: u32,
    pub sw: u32,
    /// CSS width of the block and of its server part.
    pub wc: String,
    pub swc: String,
    pub hit: bool,
    /// The newest freeze of the graph.
    pub newest: bool,
    /// The share of server wait: "65%".
    pub pct: String,
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
pub struct Lane {
    pub label: String,
    pub url: String,
    pub total: String,
    pub n: String,
    pub off: bool,
    pub sess: Vec<Seg>,
    pub blocks: Vec<Block>,
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
    /// The REC label goes to the left of the playhead (near the right end).
    pub rec_left: bool,
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
    /// Positions and widths to time scale (zoom of 4 h or less, the strip).
    pub scaled: bool,
    pub zoom_links: Vec<ZLink>,
    /// "Reset zoom" link, when the window is not the day.
    pub zoom_reset: String,
    /// "15:39–15:54 CST" when zoomed.
    pub zoom_label: String,
    /// The window in minutes of the day, for drag zoom in app.js.
    pub lo_min: String,
    pub hi_min: String,
    pub latest: Option<Latest>,
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
    pub zoom: Zoom,
    /// The page query without `zoom` and `span`, for the zoom links.
    pub keep_qs: String,
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

/// "HH:MM" (or "HH:MM:SS") of minutes of the day.
fn hm(m: f64, secs: bool) -> String {
    let t = (m * 60.0).round().max(0.0) as i64;
    if secs {
        format!("{:02}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
    } else {
        format!("{:02}:{:02}", t / 3600, (t / 60) % 60)
    }
}

/// Axis ticks of a window in minutes of the day: about 8 labels when wide,
/// 4 when narrow. Ticks close to the playhead are left out or hidden.
fn ticks(lo: f64, hi: f64, playhead: bool, now_m: f64) -> Vec<Tick> {
    const STEPS: [i64; 15] = [
        5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 14400, 21600,
    ];
    let span = (hi - lo) * 60.0;
    let pick = |n: f64| {
        STEPS
            .iter()
            .copied()
            .find(|s| span / *s as f64 <= n)
            .unwrap_or(21600)
    };
    let (s1, s2) = (pick(8.0), pick(4.0));
    let secs = s1 < 60;
    let win = Win { lo, hi };
    let mut out = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for step in [s1, s2] {
        let mut t = ((lo * 60.0) / step as f64).ceil() as i64 * step;
        while (t as f64) <= hi * 60.0 {
            if seen.insert(t) {
                let m = t as f64 / 60.0;
                let near_now = playhead && (m - now_m).abs() < (hi - lo) * 0.04;
                // The REC label is about 150 px wide: up to a quarter of the window.
                let under_rec = playhead && m > now_m && m - now_m <= (hi - lo) * 0.25;
                if !near_now {
                    out.push((
                        win.x(m),
                        Tick {
                            x: pc(win.x(m)),
                            label: hm(m, secs),
                            wide: t % s1 == 0,
                            narrow: t % s2 == 0,
                            near: under_rec,
                        },
                    ));
                }
            }
            t += step;
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|(_, t)| t).collect()
}

pub fn build(ctx: &Ctx, inp: Input) -> Lanes {
    let z = ctx.z;
    let now = ctx.now;
    let today = z.date_of(now);
    let now_m = tod(z, now);
    let has_today = inp.r.from <= today && today <= inp.r.to;
    // The zoom window, when the window does not come from the data.
    let zwin: Option<(f64, f64)> = match inp.zoom {
        Zoom::Day => None,
        Zoom::Last(x) if has_today => Some((
            (now_m - x).max(0.0),
            (now_m + (x * 0.06).max(0.5)).min(24.0 * 60.0),
        )),
        Zoom::Last(_) => None,
        Zoom::Span(a, b) => Some((a, b)),
    };
    let mut evs = events(inp.facts);
    if let Some((a, b)) = zwin {
        evs.retain(|e| {
            let m = tod(z, e.ts);
            m + e.d as f64 / 60_000.0 >= a && m < b
        });
    }
    let mut out = Lanes {
        id: inp.id.into(),
        title: inp.title.into(),
        mode_days: inp.mode == Mode::Days,
        all_url: inp.all_url.clone(),
        now_ms: now,
        now_label: z.hms(now),
        ..Default::default()
    };
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
                            sess_title(z, s.start, s.end, s.open),
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
                        (tod(z, a), tod(z, b), sess_title(z, s.start, s.end, s.open))
                    })
                    .collect();
                let e = by_day.remove(&d).unwrap_or_default();
                raws.push(Raw {
                    label: d.format("%a, %b %-d").to_string(),
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
    let (lo, hi) = if let Some(w) = zwin {
        w
    } else {
        let mut lo = 8.0 * 60.0;
        let mut hi = 18.0 * 60.0;
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
        (
            ((lo / 60.0).floor() * 60.0).max(0.0),
            ((hi / 60.0).ceil() * 60.0).min(24.0 * 60.0),
        )
    };
    let win = Win { lo, hi };
    let span_min = (hi - lo).max(MIN_SPAN);
    out.scaled = zwin.is_some() && span_min <= SCALE_MIN;
    out.win0 = lo.floor() as i64;
    out.win1 = (lo.floor() + span_min.ceil()) as i64;
    out.lo_min = format!("{lo:.3}");
    out.hi_min = format!("{hi:.3}");
    let playhead = inp.mode == Mode::Clients && has_today && now_m >= lo && now_m <= hi;
    if playhead {
        out.now_x = pc(win.x(now_m));
        out.now_f = format!("{:.5}", win.x(now_m).clamp(0.0, 100.0) / 100.0);
        out.rec_left = win.x(now_m) > 70.0;
        out.win1 = out.win0 + span_min.round().max(1.0) as i64;
    }
    out.axis = ticks(lo, hi, playhead, now_m);

    // Zoom links. "Last N" needs today in the range.
    let link = |extra: &str| -> String {
        let mut q = inp.keep_qs.clone();
        if !extra.is_empty() {
            if !q.is_empty() {
                q.push('&');
            }
            q.push_str(extra);
        }
        format!("?{q}")
    };
    let key = inp.zoom.key();
    if has_today {
        for (k, label) in [("15m", "15 min"), ("1h", "1 h"), ("4h", "4 h")] {
            out.zoom_links.push(ZLink {
                label: label.into(),
                href: link(&format!("zoom={k}")),
                on: key == k,
            });
        }
    }
    out.zoom_links.push(ZLink {
        label: "Day".into(),
        href: link(""),
        on: key == "day" || zwin.is_none(),
    });
    if zwin.is_some() {
        out.zoom_reset = link("");
        let at = if has_today {
            now
        } else {
            z.day_start(inp.r.to)
        };
        out.zoom_label = format!(
            "{}–{} {}",
            hm(lo, span_min < 10.0),
            hm(hi, span_min < 10.0),
            z.abbr(at)
        );
    }

    // The longest freeze of the graph gets the ring; the newest gets the
    // "newest" mark.
    let longest = raws
        .iter()
        .flat_map(|l| l.evs.iter())
        .max_by_key(|e| (e.d, std::cmp::Reverse(e.ts)))
        .map(|e| e.id);
    let newest = raws
        .iter()
        .flat_map(|l| l.evs.iter())
        .max_by_key(|e| (e.ts, e.id))
        .cloned();
    if let Some(e) = &newest {
        out.latest = Some(Latest {
            t: z.stamp_tz(e.ts, now),
            iso: crate::tz::iso(e.ts),
            ago: crate::tz::ago(e.ts, now),
            layout: e.layout.clone(),
            acct: acct_label(e),
            client: ctx.client_name(e.client),
            d: dur1(e.d),
        });
    }
    let newest = newest.map(|e| e.id);
    let total_ev: usize = raws.iter().map(|l| l.evs.len()).sum();
    let per_lane = MAX_BLOCKS_LANE.min((MAX_BLOCKS / raws.len().max(1)).max(30));
    let scaled = out.scaled;
    // Block width: glyph pixels in the day view, time scale at a close zoom.
    let widths = |m: f64, d: u64, share: f64| -> (u32, u32, String, String) {
        let w = glyph_px(d);
        let sw = server_px(w, share);
        if scaled {
            let x = win.x(m).clamp(0.0, 100.0);
            let wp = (d as f64 / 60_000.0 / span_min * 100.0).min(100.0 - x);
            (
                w,
                sw,
                format!("max(4px, {wp:.4}%)"),
                format!("{:.1}%", share * 100.0),
            )
        } else {
            (w, sw, format!("{w}px"), format!("{sw}px"))
        }
    };
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
            .map(|(a, b, t)| (a.max(lo), b.min(hi), t))
            .filter(|(a, b, _)| b > a)
            .map(|(a, b, t)| {
                let x0 = win.x(a);
                Seg {
                    x: pc(x0),
                    w: pc((win.x(b) - x0).max(0.2)),
                    title: t.clone(),
                }
            })
            .collect();
        if let Some(d) = raw.day {
            if d == today && now_m >= lo && now_m <= hi {
                lane.now_x = pc(win.x(now_m));
            }
        }
        if inp.mode == Mode::Clients && playhead {
            lane.future_x = out.now_x.clone();
        }
        if raw.evs.len() <= per_lane {
            for e in &raw.evs {
                let m = tod(z, e.ts);
                let share = if e.d > 0 {
                    e.wait as f64 / e.d as f64
                } else {
                    0.0
                };
                let (w, sw, wc, swc) = widths(m, e.d, share);
                let client = ctx.client_name(e.client);
                let when = z.stamp_tz(e.ts, now);
                let mut acct = acct_label(e);
                if !e.user.is_empty() && e.user != e.acct {
                    acct = format!("{acct} {}", e.user);
                }
                lane.blocks.push(Block {
                    id: e.id,
                    x_v: win.x(m),
                    x: pc(win.x(m)),
                    w,
                    sw,
                    wc,
                    swc,
                    hit: Some(e.id) == longest,
                    newest: Some(e.id) == newest,
                    pct: pct(share),
                    aria: format!(
                        "{when}, {client}, {}, {}, {}, {} server wait",
                        e.layout,
                        script_label(&e.script),
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
            let step = span_min / per_lane as f64;
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
                let label = if v.len() == 1 {
                    String::new()
                } else {
                    format!(
                        "{} freezes {}–{}",
                        v.len(),
                        hm(m0, step < 1.0),
                        hm(m1, step < 1.0)
                    )
                };
                let mt = tod(z, top.ts);
                let (w, sw, wc, swc) = widths(mt, top.d, share);
                let client = ctx.client_name(top.client);
                let when = z.stamp_tz(top.ts, now);
                lane.blocks.push(Block {
                    id: top.id,
                    x_v: win.x(mt),
                    x: pc(win.x(mt)),
                    w,
                    sw,
                    wc,
                    swc,
                    hit: v.iter().any(|e| Some(e.id) == longest),
                    newest: v.iter().any(|e| Some(e.id) == newest),
                    pct: pct(share),
                    aria: if label.is_empty() {
                        format!(
                            "{when}, {client}, {}, {}, {}, {} server wait",
                            top.layout,
                            script_label(&top.script),
                            dur1(top.d),
                            pct(share)
                        )
                    } else {
                        format!(
                            "{label} · longest {} at {when} · total {}",
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
        out.lanes.push(lane);
    }

    // Caption and table view.
    let all_evs: Vec<&Ev> = raws.iter().flat_map(|l| l.evs.iter()).collect();
    out.empty = all_evs.is_empty();
    let top = all_evs
        .iter()
        .max_by_key(|e| (e.d, std::cmp::Reverse(e.ts)))
        .copied();
    let freezes = |n: usize| if n == 1 { "freeze" } else { "freezes" };
    match (inp.mode, top) {
        (_, Some(t)) if zwin.is_some() => {
            out.caption = format!(
                "{} {} from {}. Longest {}, {}, on {} ({}, {}).",
                num(total_ev as u64),
                freezes(total_ev),
                out.zoom_label.replace('–', " to "),
                dur1(t.d),
                z.stamp_tz(t.ts, now),
                ctx.client_name(t.client),
                t.layout,
                acct_label(t)
            );
        }
        (_, None) if zwin.is_some() => {
            out.caption = format!("No freezes from {}.", out.zoom_label.replace('–', " to "));
        }
        (Mode::Clients, Some(t)) => {
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
                "{} {} on {} {} since {} {}. Longest {}, {}, on {} ({}, {}).",
                num(total_ev as u64),
                freezes(total_ev),
                nclients,
                if nclients == 1 { "client" } else { "clients" },
                z.fmt(first, "%H:%M"),
                z.abbr(first),
                dur1(t.d),
                z.stamp_tz(t.ts, now),
                ctx.client_name(t.client),
                t.layout,
                acct_label(t)
            );
        }
        (Mode::Clients, None) => {
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
                    Some(t) => format!("last event {}", z.stamp_tz(t, now)),
                    None => "no events yet".into(),
                }
            );
        }
        (Mode::Days, Some(t)) => {
            let ndays = raws.iter().filter(|l| !l.evs.is_empty()).count();
            out.caption = format!(
                "{} {} on {} {}. Longest {}, {}, on {} ({}, {}).",
                num(total_ev as u64),
                freezes(total_ev),
                ndays,
                if ndays == 1 { "day" } else { "days" },
                dur1(t.d),
                z.stamp_tz(t.ts, now),
                ctx.client_name(t.client),
                t.layout,
                acct_label(t)
            );
        }
        (Mode::Days, None) => {
            out.caption = if inp.r.kind == RangeKind::Today {
                "No freezes yet today.".into()
            } else {
                "No freezes in this range.".into()
            };
        }
        (Mode::Strip, _) => {}
    }
    let mut rows: Vec<&Ev> = all_evs.clone();
    rows.sort_by_key(|e| std::cmp::Reverse((e.ts, e.id)));
    out.table_more = rows.len() > TABLE_ROWS;
    out.table = rows
        .into_iter()
        .take(TABLE_ROWS)
        .map(|e| TRow {
            t: z.stamp(e.ts, now),
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
        if inp.mode == Mode::Clients && has_today && inp.r.kind == RangeKind::Today {
            out.live_url = format!("/fragment/lanes?{qs}");
        }
    }
    out
}

fn sess_title(z: Zone, start: i64, end: i64, open: bool) -> String {
    format!(
        "Session {}–{} {}",
        z.fmt(start, "%H:%M"),
        if open {
            "open".to_string()
        } else {
            z.fmt(end, "%H:%M")
        },
        z.abbr(start)
    )
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
        let wp = (e.d as f64 / span * 100.0).min(100.0 - x(e.ts).clamp(0.0, 100.0));
        lane.blocks.push(Block {
            id: e.id,
            x_v: x(e.ts),
            x: pc(x(e.ts)),
            w,
            sw: server_px(w, share),
            wc: format!("max(4px, {wp:.4}%)"),
            swc: format!("{:.1}%", share * 100.0),
            hit: e.id == id,
            newest: false,
            pct: pct(share),
            aria: format!(
                "{}, {}, {}, {}, {} server wait{}",
                z.stamp_tz(e.ts, ctx.now),
                e.layout,
                script_label(&e.script),
                dur1(e.d),
                pct(share),
                if e.id == id { ", this freeze" } else { "" }
            ),
            t: z.stamp_tz(e.ts, ctx.now),
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
            near: false,
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
            "{} {} on {} from {} to {} {}.",
            num(n as u64),
            if n == 1 { "freeze" } else { "freezes" },
            lane.label,
            z.fmt(w0, "%H:%M"),
            z.fmt(w1, "%H:%M"),
            z.abbr(at)
        ),
        scaled: true,
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
    fn zoom_from_the_query() {
        assert_eq!(Zoom::parse("", ""), Zoom::Day);
        assert_eq!(Zoom::parse("1h", ""), Zoom::Last(60.0));
        assert_eq!(Zoom::parse("15m", ""), Zoom::Last(15.0));
        assert_eq!(Zoom::parse("bad", ""), Zoom::Day);
        // span wins, the order of the ends does not matter, limits apply.
        assert_eq!(Zoom::parse("1h", "630-600"), Zoom::Span(600.0, 630.0));
        assert_eq!(Zoom::parse("", "-5-2000"), Zoom::Day);
        assert_eq!(Zoom::parse("", "1430-1500"), Zoom::Span(1430.0, 1440.0));
        // The smallest window is one minute.
        assert_eq!(Zoom::parse("", "600-600.2"), Zoom::Span(599.6, 600.6));
        assert_eq!(Zoom::parse("", "x-y"), Zoom::Day);
    }

    #[test]
    fn zoom_ticks_fit_the_window() {
        // 15 minutes: a label every 2 min (wide), 5 min (narrow).
        let t = ticks(600.0, 615.0, false, 0.0);
        let wide: Vec<&str> = t
            .iter()
            .filter(|x| x.wide)
            .map(|x| x.label.as_str())
            .collect();
        assert_eq!(
            wide,
            ["10:00", "10:02", "10:04", "10:06", "10:08", "10:10", "10:12", "10:14"]
        );
        assert!(t.iter().filter(|x| x.narrow).count() <= 5);
        // 2 minutes: seconds in the labels.
        let t = ticks(600.0, 602.0, false, 0.0);
        assert!(t.iter().any(|x| x.label == "10:00:30"));
        // The day: hours.
        let t = ticks(480.0, 1080.0, false, 0.0);
        assert!(t.iter().all(|x| x.label.ends_with(":00")));
    }

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
