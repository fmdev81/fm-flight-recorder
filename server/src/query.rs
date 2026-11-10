//! The read side: facts for a range, aggregates, series, flags.
//!
//! A "fact" is one freeze or script_busy event (raw, inside the event
//! retention) or one daily totals row (older days). All numbers come from
//! facts, so a range that crosses the retention cutoff adds up both.
//!
//! Rules:
//! - Server wait of a freeze = min(wait_ms, duration_ms). `wait_ms` can be
//!   a little longer than the freeze, because the net window runs a short
//!   grace time past its end. Client time = duration - server wait.
//! - script_busy is counted on its own and never added to frozen time.
//! - Percentiles are nearest-rank over the exact durations. When the range
//!   uses daily totals, they come from the merged duration histogram (the
//!   upper edge of the bucket, never more than the max).

use crate::app::{App, VERSION};
use crate::db::{Db, Settings};
use crate::tz::{Zone, DAY_MS, HOUR_MS};
use chrono::{Datelike, Days, NaiveDate};
use rusqlite::{params_from_iter, types::Value as Sql};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Upper edges (ms) of the duration buckets; the last bucket is "over".
pub const BUCKETS_MS: [u64; 22] = [
    300, 400, 500, 650, 800, 1000, 1300, 1600, 2000, 2500, 3200, 4000, 5000, 6500, 8000, 10000,
    13000, 16000, 20000, 30000, 60000, 120000,
];
pub const NB: usize = BUCKETS_MS.len() + 1;

pub fn bucket_of(ms: u64) -> usize {
    BUCKETS_MS.iter().position(|e| ms <= *e).unwrap_or(NB - 1)
}

/// The display buckets of "by duration" (each edge is also in BUCKETS_MS).
pub const DUR_EDGES: [u64; 5] = [500, 1000, 2000, 4000, 8000];
pub const DUR_LABELS: [&str; 6] = ["≤ 0.5 s", "0.5–1 s", "1–2 s", "2–4 s", "4–8 s", "> 8 s"];
pub const RT_EDGES: [u64; 4] = [0, 9, 49, 99];
pub const RT_LABELS: [&str; 5] = ["0", "1–9", "10–49", "50–99", "100+"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Freeze,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeKind {
    Today,
    Week,
    Custom,
}

/// A time range in whole local days; "today" ends now.
#[derive(Debug, Clone, Copy)]
pub struct Range {
    pub kind: RangeKind,
    pub from: NaiveDate,
    /// Inclusive.
    pub to: NaiveDate,
    pub start_ms: i64,
    pub end_ms: i64,
}

impl Range {
    pub fn today(z: Zone, now: i64) -> Range {
        let d = z.date_of(now);
        Range {
            kind: RangeKind::Today,
            from: d,
            to: d,
            start_ms: z.day_start(d),
            end_ms: z.day_end(d),
        }
    }

    pub fn week(z: Zone, now: i64) -> Range {
        let to = z.date_of(now);
        let from = to - Days::new(6);
        Range {
            kind: RangeKind::Week,
            from,
            to,
            start_ms: z.day_start(from),
            end_ms: z.day_end(to),
        }
    }

    pub fn custom(z: Zone, from: NaiveDate, to: NaiveDate) -> Range {
        let (from, to) = if from <= to { (from, to) } else { (to, from) };
        let to = to.min(from + Days::new(800));
        Range {
            kind: RangeKind::Custom,
            from,
            to,
            start_ms: z.day_start(from),
            end_ms: z.day_end(to),
        }
    }

    pub fn ndays(&self) -> u64 {
        (self.to - self.from).num_days() as u64 + 1
    }

    pub fn days(&self) -> Vec<NaiveDate> {
        self.from.iter_days().take(self.ndays() as usize).collect()
    }

    /// The range to compare with: the same day last week (up to the same
    /// time for today), the previous 7 days, or the previous period of the
    /// same length.
    pub fn previous(&self, z: Zone, now: i64) -> Range {
        let shift = if self.kind == RangeKind::Today {
            7
        } else {
            self.ndays()
        };
        let from = self.from - Days::new(shift);
        let to = self.to - Days::new(shift);
        let mut r = Range {
            kind: self.kind,
            from,
            to,
            start_ms: z.day_start(from),
            end_ms: z.day_end(to),
        };
        if self.kind == RangeKind::Today {
            r.end_ms = r.end_ms.min(now - 7 * DAY_MS + 1);
        }
        r
    }

    pub fn label(&self) -> String {
        if self.from == self.to {
            self.from.format("%a, %b %-d, %Y").to_string()
        } else if self.from.year() == self.to.year() {
            format!(
                "{} – {}",
                self.from.format("%b %-d"),
                self.to.format("%b %-d, %Y")
            )
        } else {
            format!(
                "{} – {}",
                self.from.format("%b %-d, %Y"),
                self.to.format("%b %-d, %Y")
            )
        }
    }

    /// Short label for compare lines: "Fri, Oct 30" or "Oct 24 – Oct 30".
    pub fn short(&self) -> String {
        if self.from == self.to {
            self.from.format("%a, %b %-d").to_string()
        } else {
            format!(
                "{} – {}",
                self.from.format("%b %-d"),
                self.to.format("%b %-d")
            )
        }
    }
}

/// Filters for the facts. All set fields must match.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub file: Option<String>,
    pub layout: Option<String>,
    pub script: Option<String>,
    pub account: Option<String>,
    pub client: Option<i64>,
}

impl Filter {
    pub fn file(f: &Option<String>) -> Filter {
        Filter {
            file: f.clone(),
            ..Default::default()
        }
    }

    fn sql(&self, cols: &mut Vec<String>, args: &mut Vec<Sql>) {
        let mut add = |c: &str, v: Sql| {
            cols.push(format!("{c} = ?"));
            args.push(v);
        };
        if let Some(v) = &self.file {
            add("file", Sql::Text(v.clone()));
        }
        if let Some(v) = &self.layout {
            add("layout", Sql::Text(v.clone()));
        }
        if let Some(v) = &self.script {
            add("script", Sql::Text(v.clone()));
        }
        if let Some(v) = &self.account {
            add("account", Sql::Text(v.clone()));
        }
        if let Some(v) = self.client {
            add("client_id", Sql::Integer(v));
        }
    }
}

#[derive(Debug, Clone)]
pub struct Fact {
    pub kind: Kind,
    /// Event id; 0 for a totals row.
    pub id: i64,
    pub ts_ms: i64,
    pub day: NaiveDate,
    pub file: String,
    pub layout: String,
    pub table: String,
    pub from: String,
    pub script: String,
    pub account: String,
    pub user: String,
    pub client_id: i64,
    pub n: u64,
    pub total: u64,
    /// Server wait, clamped to each duration.
    pub wait: u64,
    pub max: u64,
    pub rt_sum: u64,
    pub rt_max: u64,
    pub srtt: u64,
    pub hist: Option<Box<[u64; NB]>>,
}

impl Fact {
    pub fn raw(&self) -> bool {
        self.id != 0
    }
}

/// Load the facts of `r`: raw events from `cutoff` on, daily totals before.
pub fn facts(db: &Db, z: Zone, r: &Range, cutoff: NaiveDate, f: &Filter) -> Vec<Fact> {
    let mut out = Vec::new();
    let cutoff_ms = z.day_start(cutoff);
    // Raw events.
    let start = r.start_ms.max(cutoff_ms);
    if start < r.end_ms {
        let mut cols = vec![
            "type IN ('freeze', 'script_busy')".to_string(),
            "ts_ms >= ?".into(),
            "ts_ms < ?".into(),
        ];
        let mut args = vec![Sql::Integer(start), Sql::Integer(r.end_ms)];
        f.sql(&mut cols, &mut args);
        let sql = format!(
            "SELECT id, type, ts_ms, file, layout, layout_table, from_layout, script, account,
                    user, client_id, duration_ms, wait_ms, round_trips, srtt_ms
             FROM events WHERE {} ORDER BY ts_ms",
            cols.join(" AND ")
        );
        if let Ok(mut q) = db.prepare(&sql) {
            let rows = q.query_map(params_from_iter(args), |row| {
                let d: i64 = row.get(11)?;
                let w: i64 = row.get(12)?;
                let rt: i64 = row.get(13)?;
                let ts: i64 = row.get(2)?;
                let kind: String = row.get(1)?;
                Ok(Fact {
                    kind: if kind == "freeze" {
                        Kind::Freeze
                    } else {
                        Kind::Busy
                    },
                    id: row.get(0)?,
                    ts_ms: ts,
                    day: z.date_of(ts),
                    file: row.get(3)?,
                    layout: row.get(4)?,
                    table: row.get(5)?,
                    from: row.get(6)?,
                    script: row.get(7)?,
                    account: row.get(8)?,
                    user: row.get(9)?,
                    client_id: row.get(10)?,
                    n: 1,
                    total: d.max(0) as u64,
                    wait: w.clamp(0, d.max(0)) as u64,
                    max: d.max(0) as u64,
                    rt_sum: rt.max(0) as u64,
                    rt_max: rt.max(0) as u64,
                    srtt: row.get::<_, i64>(14)?.max(0) as u64,
                    hist: None,
                })
            });
            if let Ok(rows) = rows {
                out.extend(rows.flatten());
            }
        }
    }
    // Daily totals for the days before the cutoff.
    if r.from < cutoff {
        let last = r.to.min(cutoff - Days::new(1));
        let mut cols = vec!["day >= ?".to_string(), "day <= ?".into()];
        let mut args = vec![Sql::Text(r.from.to_string()), Sql::Text(last.to_string())];
        f.sql(&mut cols, &mut args);
        let sql = format!(
            "SELECT day, kind, file, layout, layout_table, script, account, user, client_id,
                    n, total_ms, wait_ms, max_ms, rt_sum, rt_max, hist
             FROM daily_totals WHERE {} ORDER BY day",
            cols.join(" AND ")
        );
        if let Ok(mut q) = db.prepare(&sql) {
            let rows = q.query_map(params_from_iter(args), |row| {
                let day: String = row.get(0)?;
                let day = day.parse::<NaiveDate>().unwrap_or(r.from);
                let kind: String = row.get(1)?;
                let hist: String = row.get(15)?;
                let hv: Vec<u64> = serde_json::from_str(&hist).unwrap_or_default();
                let mut h = Box::new([0u64; NB]);
                for (i, v) in hv.into_iter().enumerate().take(NB) {
                    h[i] = v;
                }
                Ok(Fact {
                    kind: if kind == "freeze" {
                        Kind::Freeze
                    } else {
                        Kind::Busy
                    },
                    id: 0,
                    ts_ms: z.day_start(day),
                    day,
                    file: row.get(2)?,
                    layout: row.get(3)?,
                    table: row.get(4)?,
                    from: String::new(),
                    script: row.get(5)?,
                    account: row.get(6)?,
                    user: row.get(7)?,
                    client_id: row.get(8)?,
                    n: row.get::<_, i64>(9)? as u64,
                    total: row.get::<_, i64>(10)? as u64,
                    wait: row.get::<_, i64>(11)? as u64,
                    max: row.get::<_, i64>(12)? as u64,
                    rt_sum: row.get::<_, i64>(13)? as u64,
                    rt_max: row.get::<_, i64>(14)? as u64,
                    srtt: 0,
                    hist: Some(h),
                })
            });
            if let Ok(rows) = rows {
                out.extend(rows.flatten());
            }
        }
    }
    out
}

/// Sums and percentiles of a set of facts.
#[derive(Debug, Clone, Default)]
pub struct Agg {
    pub n: u64,
    pub total: u64,
    pub wait: u64,
    pub max: u64,
    pub rt_sum: u64,
    pub rt_max: u64,
    pub durs: Vec<u64>,
    pub rts: Vec<u64>,
    pub srtts: Vec<u64>,
    pub hist: [u64; NB],
    /// True when some facts are daily totals (percentiles from the histogram).
    pub approx: bool,
    pub last_ms: i64,
}

impl Agg {
    pub fn add(&mut self, f: &Fact) {
        self.n += f.n;
        self.total += f.total;
        self.wait += f.wait;
        self.max = self.max.max(f.max);
        self.rt_sum += f.rt_sum;
        self.rt_max = self.rt_max.max(f.rt_max);
        self.last_ms = self.last_ms.max(f.ts_ms);
        match &f.hist {
            Some(h) => {
                self.approx = true;
                for (i, v) in h.iter().enumerate() {
                    self.hist[i] += v;
                }
            }
            None => {
                self.durs.push(f.total);
                self.rts.push(f.rt_sum);
                if f.srtt > 0 {
                    self.srtts.push(f.srtt);
                }
                self.hist[bucket_of(f.total)] += 1;
            }
        }
    }

    pub fn of<'a>(facts: impl IntoIterator<Item = &'a Fact>) -> Agg {
        let mut a = Agg::default();
        for f in facts {
            a.add(f);
        }
        a
    }

    pub fn client(&self) -> u64 {
        self.total - self.wait
    }

    /// Server share of the frozen time, 0..1.
    pub fn share(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.wait as f64 / self.total as f64
        }
    }

    pub fn avg_ms(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.total as f64 / self.n as f64
        }
    }

    pub fn avg_rt(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.rt_sum as f64 / self.n as f64
        }
    }

    /// Nearest-rank percentile of the durations (q in 0..1).
    pub fn pct(&self, q: f64) -> u64 {
        if self.n == 0 {
            return 0;
        }
        let rank = ((q * self.n as f64).ceil() as u64).clamp(1, self.n);
        if !self.approx {
            let mut v = self.durs.clone();
            v.sort_unstable();
            return v[(rank - 1) as usize];
        }
        let mut seen = 0;
        for (i, c) in self.hist.iter().enumerate() {
            seen += c;
            if seen >= rank {
                let edge = BUCKETS_MS.get(i).copied().unwrap_or(self.max);
                return edge.min(self.max);
            }
        }
        self.max
    }

    pub fn p50(&self) -> u64 {
        self.pct(0.5)
    }

    pub fn p95(&self) -> u64 {
        self.pct(0.95)
    }

    pub fn rt_p95(&self) -> u64 {
        nearest(&self.rts, 0.95)
    }

    pub fn srtt_p(&self, q: f64) -> u64 {
        nearest(&self.srtts, q)
    }

    /// Counts in the display duration buckets.
    pub fn dur_buckets(&self) -> [u64; 6] {
        let mut out = [0u64; 6];
        for (i, c) in self.hist.iter().enumerate() {
            let edge = BUCKETS_MS.get(i).copied().unwrap_or(u64::MAX);
            let j = DUR_EDGES.iter().filter(|e| **e < edge).count();
            out[j] += c;
        }
        out
    }

    /// Counts in the round-trip buckets (raw events only).
    pub fn rt_buckets(&self) -> [u64; 5] {
        let mut out = [0u64; 5];
        for r in &self.rts {
            let j = RT_EDGES.iter().filter(|e| *r > **e).count();
            out[j] += 1;
        }
        out
    }
}

fn nearest(v: &[u64], q: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    let rank = ((q * s.len() as f64).ceil() as usize).clamp(1, s.len());
    s[rank - 1]
}

/// One row of a grouped table.
#[derive(Debug, Clone, Default)]
pub struct Group {
    pub file: String,
    pub name: String,
    pub table: String,
    pub user: String,
    pub client_id: i64,
    pub freeze: Agg,
    pub busy: Agg,
    pub accounts: BTreeSet<String>,
    pub clients: BTreeSet<i64>,
    /// Frozen ms by layout.
    pub layouts: BTreeMap<String, u64>,
}

impl Group {
    /// The layout with the most frozen time.
    pub fn worst_layout(&self) -> String {
        self.layouts
            .iter()
            .max_by_key(|(k, v)| (**v, std::cmp::Reverse((*k).clone())))
            .map(|(k, _)| k.clone())
            .unwrap_or_default()
    }
}

/// Group facts by `key` = (file, name). Sorted by frozen time, then count.
pub fn group(facts: &[Fact], key: impl Fn(&Fact) -> (String, String)) -> Vec<Group> {
    let mut m: HashMap<(String, String), Group> = HashMap::new();
    for f in facts {
        let k = key(f);
        let g = m.entry(k.clone()).or_insert_with(|| Group {
            file: k.0.clone(),
            name: k.1.clone(),
            client_id: f.client_id,
            ..Default::default()
        });
        if !f.table.is_empty() {
            g.table = f.table.clone();
        }
        if !f.user.is_empty() && f.ts_ms >= g.freeze.last_ms.max(g.busy.last_ms) {
            g.user = f.user.clone();
        }
        g.accounts.insert(f.account.clone());
        g.clients.insert(f.client_id);
        match f.kind {
            Kind::Freeze => {
                g.freeze.add(f);
                *g.layouts.entry(f.layout.clone()).or_default() += f.total;
            }
            Kind::Busy => g.busy.add(f),
        }
    }
    let mut v: Vec<Group> = m.into_values().collect();
    v.sort_by(|a, b| {
        (b.freeze.total, b.freeze.n, b.busy.total)
            .cmp(&(a.freeze.total, a.freeze.n, a.busy.total))
            .then_with(|| a.name.cmp(&b.name))
    });
    v
}

pub fn freezes_only(facts: &[Fact]) -> impl Iterator<Item = &Fact> {
    facts.iter().filter(|f| f.kind == Kind::Freeze)
}

/// One bar of a series.
#[derive(Debug, Clone, Serialize)]
pub struct Bar {
    pub label: String,
    pub title: String,
    pub n: u64,
    pub total: u64,
    pub wait: u64,
}

/// By hour for today, from the first to the last event hour.
pub fn by_hour(facts: &[Fact], z: Zone, r: &Range, now: i64) -> Vec<Bar> {
    let fz: Vec<&Fact> = freezes_only(facts).filter(|f| f.raw()).collect();
    let start = r.start_ms;
    let first = fz.iter().map(|f| f.ts_ms).min();
    let last = fz.iter().map(|f| f.ts_ms).max();
    let (a, b) = match (first, last) {
        (Some(a), Some(b)) => (a, b),
        _ => (
            now.clamp(start, r.end_ms - 1),
            now.clamp(start, r.end_ms - 1),
        ),
    };
    let ha = ((a - start) / HOUR_MS) * HOUR_MS + start;
    let hb = ((b - start) / HOUR_MS) * HOUR_MS + start + HOUR_MS;
    z.hour_starts(ha, hb)
        .into_iter()
        .map(|h| {
            let agg = Agg::of(
                fz.iter()
                    .copied()
                    .filter(|f| f.ts_ms >= h && f.ts_ms < h + HOUR_MS),
            );
            Bar {
                label: z.fmt(h, "%H"),
                title: format!(
                    "{}–{} {}",
                    z.fmt(h, "%H:%M"),
                    z.fmt(h + HOUR_MS, "%H:%M"),
                    z.abbr(h)
                ),
                n: agg.n,
                total: agg.total,
                wait: agg.wait,
            }
        })
        .collect()
}

/// By local day over the range.
pub fn by_day(facts: &[Fact], r: &Range) -> Vec<Bar> {
    let mut m: BTreeMap<NaiveDate, Agg> = BTreeMap::new();
    for f in freezes_only(facts) {
        m.entry(f.day).or_default().add(f);
    }
    r.days()
        .into_iter()
        .map(|d| {
            let a = m.remove(&d).unwrap_or_default();
            Bar {
                label: if r.ndays() > 14 {
                    d.format("%-m/%-d").to_string()
                } else {
                    d.format("%a %-d").to_string()
                },
                title: d.format("%a, %b %-d").to_string(),
                n: a.n,
                total: a.total,
                wait: a.wait,
            }
        })
        .collect()
}

/// The main series of a page: by hour for today, else by day.
pub fn series(facts: &[Fact], z: Zone, r: &Range, now: i64) -> (&'static str, Vec<Bar>) {
    if r.kind == RangeKind::Today {
        ("Frozen time by hour", by_hour(facts, z, r, now))
    } else {
        ("Frozen time by day", by_day(facts, r))
    }
}

/// Per-day rows (Layout detail "Durations by day").
pub fn day_aggs(facts: &[Fact], r: &Range) -> Vec<(NaiveDate, Agg)> {
    let mut m: BTreeMap<NaiveDate, Agg> = BTreeMap::new();
    for f in freezes_only(facts) {
        m.entry(f.day).or_default().add(f);
    }
    r.days()
        .into_iter()
        .filter_map(|d| m.remove(&d).map(|a| (d, a)))
        .collect()
}

/// A known client.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ClientRow {
    pub id: i64,
    pub name: String,
    pub machine: String,
    pub os: String,
    pub app: String,
    pub plugin: String,
    pub ip: String,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    pub connected: bool,
    /// The latest session did not end with a `session_stop`: it is still
    /// open, or the server closed it because the client went silent.
    pub session_open: bool,
}

impl ClientRow {
    pub fn label(&self) -> String {
        if !self.name.is_empty() {
            self.name.clone()
        } else if !self.machine.is_empty() {
            self.machine.clone()
        } else {
            format!("client {}", self.id)
        }
    }
}

pub fn clients(db: &Db, now: i64, connected_ms: i64) -> Vec<ClientRow> {
    let mut q = match db.prepare(
        "SELECT id, name, machine, os, app, plugin, ip, first_seen_ms, last_seen_ms,
                (SELECT closed FROM sessions s WHERE s.client_id = clients.id
                 ORDER BY coalesce(s.start_ms, s.first_seen_ms) DESC LIMIT 1)
         FROM clients ORDER BY name, id",
    ) {
        Ok(q) => q,
        Err(_) => return vec![],
    };
    q.query_map([], |r| {
        let last: i64 = r.get(8)?;
        Ok(ClientRow {
            id: r.get(0)?,
            name: r.get(1)?,
            machine: r.get(2)?,
            os: r.get(3)?,
            app: r.get(4)?,
            plugin: r.get(5)?,
            ip: r.get(6)?,
            first_seen_ms: r.get(7)?,
            last_seen_ms: last,
            connected: last >= now - connected_ms,
            session_open: r
                .get::<_, Option<String>>(9)?
                .is_some_and(|closed| closed != "stop"),
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Compare dotted versions: is `a` older than `b`?
pub fn version_lt(a: &str, b: &str) -> bool {
    let p = |s: &str| -> Vec<u64> {
        s.split(|c: char| !c.is_ascii_digit())
            .filter(|x| !x.is_empty())
            .map(|x| x.parse().unwrap_or(0))
            .collect()
    };
    let (x, y) = (p(a), p(b));
    if x.is_empty() || y.is_empty() {
        return false;
    }
    for i in 0..x.len().max(y.len()) {
        let (u, v) = (
            x.get(i).copied().unwrap_or(0),
            y.get(i).copied().unwrap_or(0),
        );
        if u != v {
            return u < v;
        }
    }
    false
}

/// Flags: highlight only, nothing is sent.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Flags {
    /// (file, layout, p95 ms)
    pub p95: Vec<(String, String, u64)>,
    /// "Silent, session open": (client id, name, minutes silent). The
    /// latest session has no `session_stop` and nothing came for too long.
    pub quiet: Vec<(i64, String, i64)>,
    /// (client id, name, plug-in version)
    pub old: Vec<(i64, String, String)>,
}

impl Flags {
    pub fn count(&self) -> usize {
        self.p95.len() + self.quiet.len() + self.old.len()
    }
}

/// Clients seen in the last 7 days count for the client flags.
pub const FLAG_CLIENT_DAYS: i64 = 7;

pub fn flags(facts: &[Fact], clients: &[ClientRow], s: &Settings, now: i64) -> Flags {
    let mut f = Flags::default();
    let layouts = group(&freezes_only(facts).cloned().collect::<Vec<_>>(), |x| {
        (x.file.clone(), x.layout.clone())
    });
    for g in layouts {
        let p = g.freeze.p95();
        if g.freeze.n > 0 && p > s.flag_p95_ms {
            f.p95.push((g.file, g.name, p));
        }
    }
    f.p95.sort_by(|a, b| b.2.cmp(&a.2));
    let recent = now - FLAG_CLIENT_DAYS * DAY_MS;
    for c in clients.iter().filter(|c| c.last_seen_ms >= recent) {
        let quiet_ms = now - c.last_seen_ms;
        // A client whose user quit FileMaker (session_stop) is not silent.
        if c.session_open && quiet_ms > s.flag_quiet_min as i64 * 60_000 {
            f.quiet.push((c.id, c.label(), quiet_ms / 60_000));
        }
        if !c.plugin.is_empty() && version_lt(&c.plugin, &s.flag_plugin_min) {
            f.old.push((c.id, c.label(), c.plugin.clone()));
        }
    }
    f
}

/// The footer of every page (and `GET /api/v1/status`).
#[derive(Debug, Clone, Serialize)]
pub struct Footer {
    pub listen: String,
    pub url: String,
    pub clients: usize,
    pub connected: usize,
    pub last_event: String,
    pub last_event_ts: Option<String>,
    pub tz: String,
    pub tz_offset: String,
    /// "CST": the zone of every time on the pages.
    pub tz_abbr: String,
    pub server_version: String,
    pub plugin_version: String,
}

pub fn footer(app: &App, db: &Db, now: i64) -> Footer {
    let cl = clients(db, now, app.connected_ms());
    let last = crate::store::last_event_ms(db);
    let mut versions: BTreeMap<String, usize> = BTreeMap::new();
    for c in cl.iter().filter(|c| !c.plugin.is_empty()) {
        *versions.entry(c.plugin.clone()).or_default() += 1;
    }
    let plugin = versions
        .iter()
        .max_by_key(|(k, n)| (**n, (*k).clone()))
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| "—".into());
    Footer {
        listen: app.listen_text(),
        url: app.public_url(),
        clients: cl.len(),
        connected: cl.iter().filter(|c| c.connected).count(),
        last_event: last
            .map(|t| app.zone.stamp(t, now))
            .unwrap_or_else(|| "—".into()),
        last_event_ts: last.map(crate::tz::iso),
        tz: app.zone.name().to_string(),
        tz_offset: app.zone.offset_label(now),
        tz_abbr: app.zone.abbr(now),
        server_version: VERSION.to_string(),
        plugin_version: plugin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(d: u64, w: u64) -> Fact {
        Fact {
            kind: Kind::Freeze,
            id: 1,
            ts_ms: 0,
            day: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            file: "F".into(),
            layout: "L".into(),
            table: String::new(),
            from: String::new(),
            script: String::new(),
            account: "a".into(),
            user: String::new(),
            client_id: 1,
            n: 1,
            total: d,
            wait: w.min(d),
            max: d,
            rt_sum: 0,
            rt_max: 0,
            srtt: 0,
            hist: None,
        }
    }

    #[test]
    fn nearest_rank_percentiles() {
        let a = Agg::of(&(1..=100).map(|i| fact(i * 10, 0)).collect::<Vec<_>>());
        assert_eq!(a.p50(), 500);
        assert_eq!(a.p95(), 950);
        assert_eq!(a.pct(1.0), 1000);
        assert_eq!(Agg::default().p95(), 0);
    }

    #[test]
    fn wait_is_clamped_and_client_time_is_the_rest() {
        let a = Agg::of(&[fact(1000, 1200), fact(1000, 400)]);
        assert_eq!(a.total, 2000);
        assert_eq!(a.wait, 1400);
        assert_eq!(a.client(), 600);
    }

    #[test]
    fn histogram_percentile_is_the_bucket_edge() {
        let mut t = fact(0, 0);
        t.id = 0;
        t.n = 10;
        t.total = 10 * 900;
        t.max = 1200;
        let mut h = [0u64; NB];
        h[bucket_of(900)] = 9; // (800, 1000]
        h[bucket_of(1200)] = 1; // (1000, 1300]
        t.hist = Some(Box::new(h));
        let a = Agg::of(&[t]);
        assert!(a.approx);
        assert_eq!(a.p50(), 1000);
        assert_eq!(a.p95(), 1200, "never more than the max");
    }

    #[test]
    fn versions() {
        assert!(version_lt("0.0.9", "0.1.0"));
        assert!(!version_lt("0.1.0", "0.1.0"));
        assert!(!version_lt("0.2", "0.1.9"));
        assert!(version_lt("0.1", "0.1.1"));
        assert!(!version_lt("", "0.1.0"));
    }

    #[test]
    fn duration_buckets() {
        let a = Agg::of(&[fact(300, 0), fact(500, 0), fact(501, 0), fact(9000, 0)]);
        assert_eq!(a.dur_buckets(), [2, 1, 0, 0, 0, 1]);
    }
}
