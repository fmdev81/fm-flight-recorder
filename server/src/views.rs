//! View models: the data of each dashboard page, ready for the templates.
//! The numbers come from `query`; this module picks, formats and links.

use crate::app::App;
use crate::db::{Db, Settings};
use crate::fmt::{self, dur, dur1, enc, num, pct, w};
use crate::lanes::{self, Lanes, Mode};
use crate::query::{
    self, day_aggs, facts, freezes_only, group, series, Agg, Bar, ClientRow, Fact, Filter, Flags,
    Group, Kind, Range, RangeKind, DUR_LABELS, RT_LABELS,
};
use crate::tz::{Zone, DAY_MS};
use chrono::NaiveDate;
use rusqlite::{params_from_iter, types::Value as Sql, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// "No script name" in links and filters.
pub const NO_SCRIPT: &str = "~none";

// ---------------------------------------------------------------- params

/// The query string of a page.
#[derive(Debug, Clone, Default)]
pub struct Params {
    pub range: String,
    pub from: String,
    pub to: String,
    pub file: String,
    pub cmp: bool,
    pub name: String,
    pub layout: String,
    pub script: String,
    pub account: String,
    pub client: String,
    pub id: String,
    pub src: String,
    pub before: String,
    pub show: bool,
    pub shown: u64,
    /// `lanes=all`: every client lane, no cap.
    pub all_lanes: bool,
    /// The lanes zoom: `zoom=15m|1h|4h` or `span=LO-HI` (minutes of the day).
    pub zoom: String,
    pub span: String,
    /// All the query parameters, sorted by name.
    pub raw: Vec<(String, String)>,
}

impl Params {
    pub fn from_map(m: &HashMap<String, String>) -> Params {
        let g = |k: &str| m.get(k).cloned().unwrap_or_default();
        Params {
            range: g("range"),
            from: g("from"),
            to: g("to"),
            file: g("file"),
            cmp: g("cmp") == "1",
            name: g("name"),
            layout: g("layout"),
            script: g("script"),
            account: g("account"),
            client: g("client"),
            id: g("id"),
            src: g("src"),
            before: g("before"),
            show: g("show") == "1",
            shown: g("shown").parse().unwrap_or(0),
            all_lanes: g("lanes") == "all",
            zoom: g("zoom"),
            span: g("span"),
            raw: {
                let mut v: Vec<(String, String)> =
                    m.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                v.sort();
                v
            },
        }
    }

    pub fn lanes_zoom(&self) -> lanes::Zoom {
        lanes::Zoom::parse(&self.zoom, &self.span)
    }

    /// The query string without the zoom parameters (for the zoom links).
    pub fn qs_without_zoom(&self) -> String {
        self.raw
            .iter()
            .filter(|(k, _)| k != "zoom" && k != "span")
            .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
            .collect::<Vec<_>>()
            .join("&")
    }

    pub fn range(&self, z: Zone, now: i64) -> Range {
        match self.range.as_str() {
            "7d" => Range::week(z, now),
            "custom" => {
                let today = z.date_of(now);
                let to = self.to.parse::<NaiveDate>().unwrap_or(today);
                let from = self
                    .from
                    .parse::<NaiveDate>()
                    .unwrap_or(to - chrono::Days::new(6));
                Range::custom(z, from, to)
            }
            _ => Range::today(z, now),
        }
    }

    pub fn file_opt(&self) -> Option<String> {
        (!self.file.is_empty() && self.file != "all").then(|| self.file.clone())
    }
}

/// The page header controls: range, file and compare.
#[derive(Debug, Clone, Default)]
pub struct Ctl {
    pub path: String,
    pub range: String,
    pub from: String,
    pub to: String,
    pub file: String,
    pub files: Vec<String>,
    pub date: String,
    pub cmp: bool,
    /// Identity params of a detail page, kept by the controls.
    pub keep: Vec<(String, String)>,
    pub show_file: bool,
    pub show_range: bool,
}

impl Ctl {
    fn new(path: &str, p: &Params, r: &Range) -> Ctl {
        let range = match r.kind {
            RangeKind::Today => "today",
            RangeKind::Week => "7d",
            RangeKind::Custom => "custom",
        };
        Ctl {
            path: path.into(),
            range: range.into(),
            from: r.from.to_string(),
            to: r.to.to_string(),
            file: p.file_opt().unwrap_or_default(),
            date: r.label(),
            cmp: p.cmp,
            show_file: true,
            show_range: true,
            ..Default::default()
        }
    }

    /// The range part of a query string ("range=7d").
    pub fn rq(&self) -> String {
        match self.range.as_str() {
            "today" => "range=today".into(),
            "custom" => format!("range=custom&from={}&to={}", self.from, self.to),
            r => format!("range={r}"),
        }
    }

    /// Range and file.
    pub fn qs(&self) -> String {
        let mut s = self.rq();
        if !self.file.is_empty() {
            s.push_str(&format!("&file={}", enc(&self.file)));
        }
        s
    }

    fn keep_qs(&self) -> String {
        self.keep
            .iter()
            .map(|(k, v)| format!("{k}={}&", enc(v)))
            .collect()
    }

    pub fn range_url(&self, r: &str) -> String {
        let mut s = format!("{}?{}range={r}", self.path, self.keep_qs());
        if r == "custom" {
            s.push_str(&format!("&from={}&to={}", self.from, self.to));
        }
        if !self.file.is_empty() {
            s.push_str(&format!("&file={}", enc(&self.file)));
        }
        if self.cmp {
            s.push_str("&cmp=1");
        }
        s
    }

    pub fn cmp_url(&self) -> String {
        let mut s = format!("{}?{}{}", self.path, self.keep_qs(), self.qs());
        if !self.cmp {
            s.push_str("&cmp=1");
        }
        s
    }

    pub fn pressed(&self, r: &str) -> &'static str {
        if self.range == r {
            "true"
        } else {
            "false"
        }
    }

    pub fn is_custom(&self) -> bool {
        self.range == "custom"
    }

    // Links to the detail pages keep the range.
    pub fn layout_url(&self, file: &str, name: &str) -> String {
        format!(
            "/layout?file={}&name={}&{}",
            enc(file),
            enc(name),
            self.rq()
        )
    }
    pub fn script_url(&self, file: &str, name: &str) -> String {
        let n = if name.is_empty() { NO_SCRIPT } else { name };
        format!("/script?file={}&name={}&{}", enc(file), enc(n), self.rq())
    }
    pub fn user_url(&self, account: &str) -> String {
        format!("/user?account={}&{}", enc(account), self.rq())
    }
    pub fn client_url(&self, id: i64) -> String {
        format!("/client?id={id}&{}", self.rq())
    }
    pub fn freeze_url(&self, id: i64) -> String {
        format!("/freeze?id={id}")
    }
    pub fn freezes_url(&self, extra: &str) -> String {
        format!("/freezes?{}{}", self.qs(), extra)
    }
}

// ---------------------------------------------------------------- shared

/// A split bar: server wait (blue) and client time (orange).
#[derive(Debug, Clone, Default)]
pub struct Split {
    pub s: String,
    pub c: String,
    pub title: String,
}

impl Split {
    pub fn new(a: &Agg, max: u64) -> Split {
        Split {
            s: w(a.wait, max),
            c: w(a.client(), max),
            title: format!("Server {} · Client {}", dur(a.wait), dur(a.client())),
        }
    }
    fn of(total: u64, wait: u64, max: u64) -> Split {
        Split {
            s: w(wait, max),
            c: w(total.saturating_sub(wait), max),
            title: format!(
                "Server {} · Client {}",
                dur1(wait),
                dur1(total.saturating_sub(wait))
            ),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Tile {
    pub label: String,
    pub big: String,
    pub small: String,
    pub sub: String,
    pub split: Option<Split>,
    pub link: String,
    pub cmp: String,
    pub chip: String,
}

fn tile(label: &str, big: String, sub: String) -> Tile {
    Tile {
        label: label.into(),
        big,
        sub,
        ..Default::default()
    }
}

/// The chart spec that `app.js` draws with uPlot.
#[derive(Debug, Clone, Serialize)]
pub struct Chart {
    pub kind: &'static str,
    pub unit: &'static str,
    pub labels: Vec<String>,
    pub titles: Vec<String>,
    pub n: Vec<u64>,
    pub srv: Vec<u64>,
    pub cli: Vec<u64>,
    pub height: u32,
}

impl Chart {
    pub fn stack(bars: &[Bar], height: u32) -> Chart {
        Chart {
            kind: "stack",
            unit: "ms",
            labels: bars.iter().map(|b| b.label.clone()).collect(),
            titles: bars.iter().map(|b| b.title.clone()).collect(),
            n: bars.iter().map(|b| b.n).collect(),
            srv: bars.iter().map(|b| b.wait).collect(),
            cli: bars.iter().map(|b| b.total - b.wait).collect(),
            height,
        }
    }
    pub fn counts(
        labels: Vec<String>,
        titles: Vec<String>,
        n: Vec<u64>,
        unit: &'static str,
        height: u32,
    ) -> Chart {
        Chart {
            kind: "count",
            unit,
            labels,
            titles,
            srv: vec![],
            cli: vec![],
            n,
            height,
        }
    }
    pub fn json(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
    pub fn is_empty(&self) -> bool {
        self.n.iter().all(|v| *v == 0) && self.srv.iter().chain(&self.cli).all(|v| *v == 0)
    }
}

/// A small histogram drawn with CSS bars.
#[derive(Debug, Clone, Default)]
pub struct Hist {
    pub bars: Vec<HBar>,
}

#[derive(Debug, Clone, Default)]
pub struct HBar {
    pub label: String,
    pub n: String,
    pub h: String,
    pub title: String,
}

impl Hist {
    pub fn new(labels: &[&str], counts: &[u64], unit: &str) -> Hist {
        let max = counts.iter().copied().max().unwrap_or(0);
        Hist {
            bars: labels
                .iter()
                .zip(counts)
                .map(|(l, c)| HBar {
                    label: l.to_string(),
                    n: num(*c),
                    h: w(*c, max),
                    title: format!("{l} · {} {unit}", num(*c)),
                })
                .collect(),
        }
    }
    pub fn total(&self) -> bool {
        self.bars.iter().any(|b| b.n != "0")
    }
}

/// A row of a small "name / split / count / total" table.
#[derive(Debug, Clone, Default)]
pub struct NRow {
    pub name: String,
    pub sub: String,
    pub url: String,
    pub mono: bool,
    pub muted: bool,
    pub split: Split,
    pub n: String,
    pub total: String,
    pub pct: String,
    pub p95: String,
    pub flag: bool,
}

/// A freeze in a list.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FRow {
    pub id: i64,
    pub url: String,
    pub t: String,
    pub file: String,
    pub layout: String,
    pub layout_url: String,
    pub from: String,
    pub script: String,
    pub script_url: String,
    /// A script ran during the freeze (the overview leaves the cell empty
    /// when none did).
    #[serde(skip)]
    pub has_script: bool,
    pub account: String,
    pub user: String,
    pub user_url: String,
    pub client: String,
    pub client_url: String,
    pub kind: String,
    pub level: String,
    pub dur: String,
    pub rt: String,
    pub pct: String,
    pub srv_w: String,
    pub cli_w: String,
    pub split_title: String,
    pub marks: String,
    /// The time with the zone, for the tooltip.
    #[serde(skip)]
    pub t_tz: String,
    /// "2 min ago" for a time of today, else empty.
    #[serde(skip)]
    pub ago: String,
    #[serde(skip)]
    pub iso: String,
    #[serde(skip)]
    pub ts_ms: i64,
    #[serde(skip)]
    pub dur_ms: u64,
    #[serde(skip)]
    pub wait_ms: u64,
}

fn person(account: &str) -> String {
    if account.is_empty() {
        "—".into()
    } else {
        account.into()
    }
}

fn script_label(s: &str) -> String {
    if s.is_empty() {
        "—".into()
    } else {
        s.into()
    }
}

/// Everything a page needs to read: the time, zone, settings, clients.
pub struct Ctx<'a> {
    pub app: &'a App,
    pub db: &'a Db,
    pub now: i64,
    pub z: Zone,
    pub s: Settings,
    pub cutoff: NaiveDate,
    pub clients: Vec<ClientRow>,
}

impl<'a> Ctx<'a> {
    pub fn new(app: &'a App, db: &'a Db, now: i64) -> Ctx<'a> {
        let s = app.settings(db);
        let cutoff = app.cutoff_day(&s, now);
        Ctx {
            app,
            db,
            now,
            z: app.zone,
            clients: query::clients(db, now, app.connected_ms()),
            s,
            cutoff,
        }
    }

    pub fn facts(&self, r: &Range, f: &Filter) -> Vec<Fact> {
        facts(self.db, self.z, r, self.cutoff, f)
    }

    pub fn client(&self, id: i64) -> Option<&ClientRow> {
        self.clients.iter().find(|c| c.id == id)
    }

    pub fn client_name(&self, id: i64) -> String {
        self.client(id)
            .map(|c| c.label())
            .unwrap_or_else(|| format!("client {id}"))
    }

    /// The files seen in the last 30 days, for the file select.
    pub fn files(&self) -> Vec<String> {
        let since = self.now - 30 * DAY_MS;
        let mut q = match self.db.prepare(
            "SELECT DISTINCT file FROM events WHERE ts_ms >= ?1 AND file <> '' ORDER BY file",
        ) {
            Ok(q) => q,
            Err(_) => return vec![],
        };
        q.query_map([since], |r| r.get::<_, String>(0))
            .map(|r| r.flatten().collect())
            .unwrap_or_default()
    }

    /// The one time format: "Today 16:58:12", "Nov 6, 16:58:12".
    pub fn when(&self, ms: i64) -> String {
        self.z.stamp(ms, self.now)
    }

    /// `when` with the zone, for tooltips: "Today 16:58:12 CST".
    pub fn when_tz(&self, ms: i64) -> String {
        self.z.stamp_tz(ms, self.now)
    }

    pub fn ctl(&self, path: &str, p: &Params, r: &Range) -> Ctl {
        let mut c = Ctl::new(path, p, r);
        c.files = self.files();
        c
    }

    pub fn flags(&self, r: &Range, file: &Option<String>) -> Flags {
        let f = self.facts(r, &Filter::file(file));
        query::flags(&f, &self.clients, &self.s, self.now)
    }
}

fn nrows(groups: &[Group], ctl: &Ctl, kind: &str, limit: usize, ctx: &Ctx) -> Vec<NRow> {
    let max = groups.iter().map(|g| g.freeze.total).max().unwrap_or(0);
    groups
        .iter()
        .filter(|g| g.freeze.n > 0)
        .take(limit)
        .map(|g| {
            let (name, url, mono, sub) = match kind {
                "layout" => (
                    g.name.clone(),
                    ctl.layout_url(&g.file, &g.name),
                    false,
                    g.file.clone(),
                ),
                "script" => (
                    if g.name.is_empty() {
                        "No script name".into()
                    } else {
                        g.name.clone()
                    },
                    ctl.script_url(&g.file, &g.name),
                    false,
                    g.file.clone(),
                ),
                "user" => (person(&g.name), ctl.user_url(&g.name), true, g.user.clone()),
                "client" => {
                    let id = g.name.parse().unwrap_or(0);
                    (
                        ctx.client_name(id),
                        ctl.client_url(id),
                        false,
                        String::new(),
                    )
                }
                _ => (g.name.clone(), String::new(), false, String::new()),
            };
            let p95 = g.freeze.p95();
            NRow {
                muted: kind == "script" && g.name.is_empty(),
                name,
                sub,
                url,
                mono,
                split: Split::new(&g.freeze, max),
                n: num(g.freeze.n),
                total: dur(g.freeze.total),
                pct: pct(g.freeze.share()),
                p95: dur(p95),
                flag: kind == "layout" && p95 > ctx.s.flag_p95_ms,
            }
        })
        .collect()
}

/// Freeze rows from the database, newest first.
#[derive(Debug, Clone, Default)]
pub struct FreezeQuery {
    pub start_ms: i64,
    pub end_ms: i64,
    pub filter: Filter,
    /// Event types (default: freeze).
    pub types: Vec<&'static str>,
    /// Only rows older than (ts_ms, id).
    pub before: Option<(i64, i64)>,
    pub limit: usize,
}

pub fn freeze_rows(ctx: &Ctx, ctl: &Ctl, q: &FreezeQuery) -> (Vec<FRow>, u64) {
    let types = if q.types.is_empty() {
        vec!["freeze"]
    } else {
        q.types.clone()
    };
    let mut cols = vec![
        format!(
            "type IN ({})",
            types
                .iter()
                .map(|t| format!("'{t}'"))
                .collect::<Vec<_>>()
                .join(",")
        ),
        "ts_ms >= ?".into(),
        "ts_ms < ?".into(),
    ];
    let mut args = vec![Sql::Integer(q.start_ms), Sql::Integer(q.end_ms)];
    let mut fcols = vec![];
    let mut fargs = vec![];
    filter_sql(&q.filter, &mut fcols, &mut fargs);
    cols.extend(fcols);
    args.extend(fargs);
    let total: u64 = {
        let sql = format!("SELECT count(*) FROM events WHERE {}", cols.join(" AND "));
        ctx.db
            .query_row(&sql, params_from_iter(args.clone()), |r| r.get::<_, i64>(0))
            .unwrap_or(0) as u64
    };
    if let Some((t, id)) = q.before {
        cols.push("(ts_ms < ? OR (ts_ms = ? AND id < ?))".into());
        args.extend([Sql::Integer(t), Sql::Integer(t), Sql::Integer(id)]);
    }
    let sql = format!(
        "SELECT id, ts_ms, file, layout, from_layout, script, account, user, client_id,
                duration_ms, wait_ms, round_trips, type, level, marks, label
         FROM events WHERE {} ORDER BY ts_ms DESC, id DESC LIMIT {}",
        cols.join(" AND "),
        q.limit.max(1)
    );
    let mut out = Vec::new();
    if let Ok(mut st) = ctx.db.prepare(&sql) {
        let rows = st.query_map(params_from_iter(args), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, i64>(9)?,
                r.get::<_, i64>(10)?,
                r.get::<_, i64>(11)?,
                r.get::<_, String>(12)?,
                r.get::<_, String>(13)?,
                r.get::<_, String>(14)?,
                r.get::<_, String>(15)?,
            ))
        });
        if let Ok(rows) = rows {
            for (
                id,
                ts,
                file,
                layout,
                from,
                script,
                account,
                user,
                cid,
                d,
                wt,
                rt,
                kind,
                level,
                marks,
                label,
            ) in rows.flatten()
            {
                let d = d.max(0) as u64;
                let wt = (wt.max(0) as u64).min(d);
                let marks: Vec<String> = serde_json::from_str(&marks).unwrap_or_default();
                out.push(FRow {
                    id,
                    url: if kind == "freeze" {
                        ctl.freeze_url(id)
                    } else {
                        String::new()
                    },
                    t: ctx.when(ts),
                    t_tz: ctx.when_tz(ts),
                    ago: if ctx.z.date_of(ts) == ctx.z.date_of(ctx.now) {
                        crate::tz::ago(ts, ctx.now)
                    } else {
                        String::new()
                    },
                    iso: crate::tz::iso(ts),
                    layout_url: ctl.layout_url(&file, &layout),
                    script_url: if kind == "mark" && script.is_empty() {
                        String::new()
                    } else {
                        ctl.script_url(&file, &script)
                    },
                    user_url: ctl.user_url(&account),
                    client_url: ctl.client_url(cid),
                    client: ctx.client_name(cid),
                    file,
                    layout,
                    from,
                    has_script: !script.is_empty(),
                    script: if kind == "mark" && script.is_empty() {
                        "—".into()
                    } else {
                        script_label(&script)
                    },
                    account: person(&account),
                    user,
                    kind: kind.clone(),
                    level,
                    dur: if kind == "mark" { label } else { dur1(d) },
                    rt: if kind == "mark" {
                        String::new()
                    } else {
                        num(rt.max(0) as u64)
                    },
                    pct: if kind == "mark" || d == 0 {
                        String::new()
                    } else {
                        pct(wt as f64 / d as f64)
                    },
                    srv_w: String::new(),
                    cli_w: String::new(),
                    split_title: String::new(),
                    marks: marks.join(" · "),
                    ts_ms: ts,
                    dur_ms: d,
                    wait_ms: wt,
                });
            }
        }
    }
    let max = out.iter().map(|r| r.dur_ms).max().unwrap_or(0);
    for r in out.iter_mut() {
        let sp = Split::of(r.dur_ms, r.wait_ms, max);
        r.srv_w = sp.s;
        r.cli_w = sp.c;
        r.split_title = sp.title;
    }
    (out, total)
}

fn filter_sql(f: &Filter, cols: &mut Vec<String>, args: &mut Vec<Sql>) {
    let mut add = |c: &str, v: Sql| {
        cols.push(format!("{c} = ?"));
        args.push(v);
    };
    if let Some(v) = &f.file {
        add("file", Sql::Text(v.clone()));
    }
    if let Some(v) = &f.layout {
        add("layout", Sql::Text(v.clone()));
    }
    if let Some(v) = &f.script {
        add("script", Sql::Text(v.clone()));
    }
    if let Some(v) = &f.account {
        add("account", Sql::Text(v.clone()));
    }
    if let Some(v) = f.client {
        add("client_id", Sql::Integer(v));
    }
}

fn cmp_line(prev: &Range, cur: f64, old: f64, text: String) -> String {
    format!("{} {} · {}", prev.short(), text, fmt::change(cur, old))
}

/// The five detail tiles with an optional compare line.
fn detail_tiles(cur: &Agg, old: Option<(&Range, &Agg)>, ctx: &Ctx, layout_flag: bool) -> Vec<Tile> {
    let mut t1 = tile(
        "Freezes",
        num(cur.n),
        format!("avg {}", dur1(cur.avg_ms() as u64)),
    );
    let mut t2 = tile(
        "Time frozen",
        dur(cur.total),
        format!("max {}", dur1(cur.max)),
    );
    let mut t3 = tile(
        "Waiting on server",
        pct(cur.share()),
        format!("Server {} · Client {}", dur(cur.wait), dur(cur.client())),
    );
    t3.split = Some(Split::new(cur, cur.total));
    let mut t4 = tile(
        "p50 / p95",
        format!("{} / {}", dur(cur.p50()), dur(cur.p95())),
        format!("max {}", dur1(cur.max)),
    );
    if layout_flag && cur.n > 0 && cur.p95() > ctx.s.flag_p95_ms {
        t4.chip = format!("p95 > {}", dur(ctx.s.flag_p95_ms));
    }
    let mut t5 = tile(
        "Round trips",
        format!("{} avg", fmt::avg(cur.avg_rt())),
        format!("p95 {} · max {}", num(cur.rt_p95()), num(cur.rt_max)),
    );
    if let Some((r, o)) = old {
        t1.cmp = cmp_line(r, cur.n as f64, o.n as f64, num(o.n));
        t2.cmp = cmp_line(r, cur.total as f64, o.total as f64, dur(o.total));
        t3.cmp = format!(
            "{} {} · {}",
            r.short(),
            pct(o.share()),
            fmt::change_pt(cur.share(), o.share())
        );
        t4.cmp = format!("{} {} / {}", r.short(), dur(o.p50()), dur(o.p95()));
        t5.cmp = format!("{} {} avg", r.short(), fmt::avg(o.avg_rt()));
    }
    vec![t1, t2, t3, t4, t5]
}

// ---------------------------------------------------------------- overview

pub struct FlagItem {
    pub name: String,
    pub url: String,
    pub chip: String,
}

pub struct FlagsView {
    pub count: usize,
    pub p95_label: String,
    pub quiet_label: String,
    pub plugin_label: String,
    pub p95: Vec<FlagItem>,
    pub quiet: Vec<FlagItem>,
    pub old: Vec<FlagItem>,
}

fn flags_view(f: &Flags, ctl: &Ctl, s: &Settings) -> FlagsView {
    FlagsView {
        count: f.count(),
        p95_label: format!("p95 over {}", dur(s.flag_p95_ms)),
        quiet_label: format!("Silent, session open over {} min", s.flag_quiet_min),
        plugin_label: format!("Plug-in older than {}", s.flag_plugin_min),
        p95: f
            .p95
            .iter()
            .map(|(file, l, p)| FlagItem {
                name: l.clone(),
                url: ctl.layout_url(file, l),
                chip: format!("p95 {}", dur(*p)),
            })
            .collect(),
        quiet: f
            .quiet
            .iter()
            .map(|(id, n, m)| FlagItem {
                name: n.clone(),
                url: ctl.client_url(*id),
                chip: quiet_text(*m),
            })
            .collect(),
        old: f
            .old
            .iter()
            .map(|(id, n, v)| FlagItem {
                name: n.clone(),
                url: ctl.client_url(*id),
                chip: v.clone(),
            })
            .collect(),
    }
}

fn quiet_text(min: i64) -> String {
    if min < 120 {
        format!("silent {min} min")
    } else if min < 48 * 60 {
        format!("silent {} h", min / 60)
    } else {
        format!("silent {} d", min / (24 * 60))
    }
}

/// Clients-mode lanes for today (Overview Today, Clients, the live fragment).
pub fn today_lanes(ctx: &Ctx, p: &Params) -> Lanes {
    let r = Range::today(ctx.z, ctx.now);
    let file = p.file_opt();
    let f = ctx.facts(&r, &Filter::file(&file));
    let sess = sessions_between(ctx, r.start_ms, r.end_ms, None);
    let mut qs = format!("range=today{}", lanes::file_qs(&file));
    if p.all_lanes {
        qs.push_str("&lanes=all");
    }
    if !p.zoom.is_empty() {
        qs.push_str(&format!("&zoom={}", enc(&p.zoom)));
    }
    if !p.span.is_empty() {
        qs.push_str(&format!("&span={}", enc(&p.span)));
    }
    lanes::build(
        ctx,
        lanes::Input {
            mode: Mode::Clients,
            title: "Who froze, when",
            id: "lanes",
            r: &r,
            facts: &f,
            sess: &sess,
            all_lanes: p.all_lanes,
            day_url: &|_| String::new(),
            all_url: format!("/freezes?range=today{}", lanes::file_qs(&file)),
            live_qs: Some(qs),
            zoom: p.lanes_zoom(),
            keep_qs: p.qs_without_zoom(),
        },
    )
}

/// Days-mode lanes for a range, or None when only daily totals exist
/// (more than 31 days, or all days past the event retention).
fn day_lanes(
    ctx: &Ctx,
    p: &Params,
    r: &Range,
    f: &[Fact],
    sess: &[SessRow],
    day_url: &dyn Fn(NaiveDate) -> String,
    all_url: String,
) -> Option<Lanes> {
    if r.ndays() > 31 || r.to < ctx.cutoff {
        return None;
    }
    Some(lanes::build(
        ctx,
        lanes::Input {
            mode: Mode::Days,
            title: "Who froze, when",
            id: "lanes",
            r,
            facts: f,
            sess,
            all_lanes: true,
            day_url,
            all_url,
            live_qs: None,
            zoom: p.lanes_zoom(),
            keep_qs: p.qs_without_zoom(),
        },
    ))
}

/// A link to one day of a page ("range=custom&from=D&to=D").
fn day_link(base: &str, d: NaiveDate) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}range=custom&from={d}&to={d}")
}

/// "none yet today" or "none in this range".
fn none_text(r: &Range) -> &'static str {
    if r.kind == RangeKind::Today {
        "none yet today"
    } else {
        "none in this range"
    }
}

/// Rows of "Latest freezes" on the overview.
pub const OVERVIEW_ROWS: usize = 8;

pub struct Overview {
    pub ctl: Ctl,
    pub tiles: Vec<Tile>,
    pub flags: FlagsView,
    pub lanes: Option<Lanes>,
    pub chart_title: String,
    pub chart: Chart,
    pub recent: Vec<FRow>,
    /// Freezes in the range, for "15 of 120".
    pub recent_total: String,
}

pub fn overview(ctx: &Ctx, p: &Params) -> Overview {
    let r = p.range(ctx.z, ctx.now);
    let ctl = ctx.ctl("/", p, &r);
    let file = p.file_opt();
    let f = ctx.facts(&r, &Filter::file(&file));
    let fz: Vec<Fact> = freezes_only(&f).cloned().collect();
    let a = Agg::of(&fz);
    let clients_hit: BTreeSet<i64> = fz.iter().map(|x| x.client_id).collect();
    let connected = ctx.clients.iter().filter(|c| c.connected).count();
    let none = none_text(&r);
    let mut t3 = if a.n == 0 {
        tile(
            "Waiting on server",
            "—".into(),
            "no freezes to split".into(),
        )
    } else {
        // The split bar is the sub-line; its tooltip has the seconds.
        tile("Waiting on server", pct(a.share()), String::new())
    };
    if a.n > 0 {
        t3.split = Some(Split::new(&a, a.total));
    }
    let mut t4 = tile(
        "Clients affected",
        num(clients_hit.len() as u64),
        // Offline clients and accounts are on the Clients and Users pages.
        format!("{connected} connected"),
    );
    t4.small = format!("of {}", ctx.clients.len());
    t4.link = "/users".into();
    let tiles = if a.n == 0 {
        vec![
            tile("Freezes", "0".into(), none.into()),
            tile("Time frozen", "—".into(), none.into()),
            t3,
            t4,
        ]
    } else {
        vec![
            tile(
                "Freezes",
                num(a.n),
                // Script busy is on the Scripts page.
                format!("p95 {}", dur(a.p95())),
            ),
            tile(
                "Time frozen",
                dur(a.total),
                format!("avg {}", dur1(a.avg_ms() as u64)),
            ),
            t3,
            t4,
        ]
    };
    let (title, bars) = series(&f, ctx.z, &r, ctx.now);
    let flags = query::flags(&f, &ctx.clients, &ctx.s, ctx.now);
    let lanes = if r.kind == RangeKind::Today {
        Some(today_lanes(ctx, p))
    } else {
        let sess = sessions_between(ctx, r.start_ms, r.end_ms, None);
        let fq = lanes::file_qs(&file);
        day_lanes(
            ctx,
            p,
            &r,
            &f,
            &sess,
            &|d| format!("{}{fq}", day_link("/", d)),
            ctl.freezes_url(""),
        )
    };
    // The newest freezes first: the main panel of the overview. The worst
    // layouts and scripts are on the Layouts and Scripts pages.
    let (recent, recent_total) = freeze_rows(
        ctx,
        &ctl,
        &FreezeQuery {
            start_ms: r.start_ms,
            end_ms: r.end_ms,
            filter: Filter::file(&file),
            limit: OVERVIEW_ROWS,
            ..Default::default()
        },
    );
    Overview {
        tiles,
        flags: flags_view(&flags, &ctl, &ctx.s),
        lanes,
        chart_title: title.into(),
        chart: Chart::stack(&bars, 160),
        recent,
        recent_total: num(recent_total),
        ctl,
    }
}

// ---------------------------------------------------------------- layouts

pub struct LRow {
    pub name: String,
    pub url: String,
    pub file: String,
    pub table: String,
    pub n: String,
    pub n_v: u64,
    pub total: String,
    pub total_v: u64,
    pub split: Split,
    pub p50: String,
    pub p95: String,
    pub p95_v: u64,
    pub flag: bool,
    pub rt: String,
    pub pct: String,
    pub pct_v: u64,
}

pub struct Layouts {
    pub ctl: Ctl,
    pub rows: Vec<LRow>,
    pub foot: LRow,
    pub flagged: usize,
}

fn lrow(
    g: &Agg,
    name: String,
    url: String,
    file: String,
    table: String,
    max: u64,
    flag_ms: u64,
) -> LRow {
    let p95 = g.p95();
    LRow {
        name,
        url,
        file,
        table,
        n: num(g.n),
        n_v: g.n,
        total: dur(g.total),
        total_v: g.total,
        split: Split::new(g, max),
        p50: dur(g.p50()),
        p95: dur(p95),
        p95_v: p95,
        flag: g.n > 0 && p95 > flag_ms,
        rt: fmt::avg(g.avg_rt()),
        pct: pct(g.share()),
        pct_v: (g.share() * 1000.0) as u64,
    }
}

pub fn layouts(ctx: &Ctx, p: &Params) -> Layouts {
    let r = p.range(ctx.z, ctx.now);
    let ctl = ctx.ctl("/layouts", p, &r);
    let file = p.file_opt();
    let fz: Vec<Fact> = freezes_only(&ctx.facts(&r, &Filter::file(&file)))
        .cloned()
        .collect();
    let gs = group(&fz, |x| (x.file.clone(), x.layout.clone()));
    let max = gs.iter().map(|g| g.freeze.total).max().unwrap_or(0);
    let rows: Vec<LRow> = gs
        .iter()
        .map(|g| {
            lrow(
                &g.freeze,
                g.name.clone(),
                ctl.layout_url(&g.file, &g.name),
                g.file.clone(),
                g.table.clone(),
                max,
                ctx.s.flag_p95_ms,
            )
        })
        .collect();
    let all = Agg::of(&fz);
    let mut foot = lrow(
        &all,
        "All".into(),
        String::new(),
        String::new(),
        String::new(),
        all.total,
        u64::MAX,
    );
    foot.flag = false;
    Layouts {
        flagged: rows.iter().filter(|r| r.flag).count(),
        rows,
        foot,
        ctl,
    }
}

pub struct LayoutDetail {
    pub ctl: Ctl,
    pub name: String,
    pub file: String,
    pub table: String,
    pub rank: String,
    pub tiles: Vec<Tile>,
    pub lanes: Option<Lanes>,
    pub chart_title: String,
    pub chart: Chart,
    pub days: Vec<DayRow>,
    pub by_dur: Hist,
    pub by_rt: Hist,
    pub users: Vec<NRow>,
    pub scripts: Vec<NRow>,
    pub from: Vec<NRow>,
    pub recent: Vec<FRow>,
}

pub struct DayRow {
    pub day: String,
    pub n: String,
    pub total: String,
    pub p50: String,
    pub p95: String,
    pub rt: String,
    pub pct: String,
}

fn day_rows(f: &[Fact], r: &Range) -> Vec<DayRow> {
    day_aggs(f, r)
        .into_iter()
        .map(|(d, a)| DayRow {
            day: d.format("%a, %b %-d").to_string(),
            n: num(a.n),
            total: dur(a.total),
            p50: dur(a.p50()),
            p95: dur(a.p95()),
            rt: fmt::avg(a.avg_rt()),
            pct: pct(a.share()),
        })
        .collect()
}

fn compare(ctx: &Ctx, p: &Params, r: &Range, f: &Filter) -> Option<(Range, Agg)> {
    if !p.cmp {
        return None;
    }
    let prev = r.previous(ctx.z, ctx.now);
    let pf = ctx.facts(&prev, f);
    Some((prev, Agg::of(freezes_only(&pf))))
}

pub fn layout_detail(ctx: &Ctx, p: &Params) -> LayoutDetail {
    let r = p.range(ctx.z, ctx.now);
    let mut ctl = ctx.ctl("/layout", p, &r);
    ctl.keep = vec![
        ("file".into(), p.file.clone()),
        ("name".into(), p.name.clone()),
    ];
    ctl.file.clear();
    ctl.show_file = false;
    let filt = Filter {
        file: Some(p.file.clone()),
        layout: Some(p.name.clone()),
        ..Default::default()
    };
    let f = ctx.facts(&r, &filt);
    let fz: Vec<Fact> = freezes_only(&f).cloned().collect();
    let a = Agg::of(&fz);
    let cmp = compare(ctx, p, &r, &filt);
    let tiles = detail_tiles(&a, cmp.as_ref().map(|(r, a)| (r, a)), ctx, true);
    // Rank among all layouts of the range.
    let all: Vec<Fact> = freezes_only(&ctx.facts(&r, &Filter::default()))
        .cloned()
        .collect();
    let gs = group(&all, |x| (x.file.clone(), x.layout.clone()));
    let rank = gs
        .iter()
        .position(|g| g.file == p.file && g.name == p.name)
        .map(|i| format!("Rank {} of {} by time frozen", i + 1, gs.len()))
        .unwrap_or_default();
    let (title, bars) = series(&f, ctx.z, &r, ctx.now);
    let users = group(&fz, |x| (String::new(), x.account.clone()));
    let scripts = group(&fz, |x| (x.file.clone(), x.script.clone()));
    let raw: Vec<Fact> = fz.iter().filter(|x| x.raw()).cloned().collect();
    let froms = group(&raw, |x| (x.file.clone(), x.from.clone()));
    let fmax = froms.iter().map(|g| g.freeze.n).max().unwrap_or(0);
    let from = froms
        .iter()
        .take(8)
        .map(|g| NRow {
            name: if g.name.is_empty() {
                "—".into()
            } else {
                g.name.clone()
            },
            url: if g.name.is_empty() {
                String::new()
            } else {
                ctl.layout_url(&g.file, &g.name)
            },
            split: Split {
                s: w(g.freeze.n, fmax),
                c: "0%".into(),
                title: String::new(),
            },
            n: num(g.freeze.n),
            total: dur(g.freeze.total),
            ..Default::default()
        })
        .collect();
    let (recent, _) = freeze_rows(
        ctx,
        &ctl,
        &FreezeQuery {
            start_ms: r.start_ms,
            end_ms: r.end_ms,
            filter: filt.clone(),
            limit: 10,
            ..Default::default()
        },
    );
    let table = fz
        .iter()
        .rev()
        .find(|x| !x.table.is_empty())
        .map(|x| x.table.clone())
        .unwrap_or_default();
    LayoutDetail {
        name: p.name.clone(),
        file: p.file.clone(),
        table,
        rank,
        tiles,
        lanes: {
            let base = format!("/layout?file={}&name={}", enc(&p.file), enc(&p.name));
            day_lanes(
                ctx,
                p,
                &r,
                &f,
                &[],
                &|d| day_link(&base, d),
                format!(
                    "/freezes?{}&file={}&layout={}",
                    ctl.rq(),
                    enc(&p.file),
                    enc(&p.name)
                ),
            )
        },
        chart_title: title.into(),
        chart: Chart::stack(&bars, 150),
        days: day_rows(&f, &r),
        by_dur: Hist::new(&DUR_LABELS, &a.dur_buckets(), "freezes"),
        by_rt: Hist::new(&RT_LABELS, &a.rt_buckets(), "freezes"),
        users: nrows(&users, &ctl, "user", 8, ctx),
        scripts: nrows(&scripts, &ctl, "script", 8, ctx),
        from,
        recent,
        ctl,
    }
}

// ---------------------------------------------------------------- scripts

pub struct SRow {
    pub name: String,
    pub url: String,
    pub file: String,
    pub tags: Vec<String>,
    pub n: String,
    pub n_v: u64,
    pub total: String,
    pub total_v: u64,
    pub split: Split,
    pub p50: String,
    pub p95: String,
    pub pct: String,
    pub runs: String,
    pub busy: String,
    pub busy_v: u64,
    pub accounts: String,
    pub layout: String,
    pub layout_url: String,
}

pub struct NoScript {
    pub n: String,
    pub total: String,
    pub pct: String,
    pub layouts: String,
    pub url: String,
}

pub struct Scripts {
    pub ctl: Ctl,
    pub tiles: Vec<Tile>,
    pub src: String,
    pub src_urls: Vec<(String, String, bool)>,
    pub rows: Vec<SRow>,
    pub none: Option<NoScript>,
}

/// Mark counts by (file, script) in a range, with the newest label.
fn marks_by_script(
    ctx: &Ctx,
    r: &Range,
    file: &Option<String>,
) -> HashMap<(String, String), (u64, String)> {
    let mut m = HashMap::new();
    let mut sql = "SELECT file, script, count(*), max(label) FROM events
                   WHERE type = 'mark' AND script <> '' AND ts_ms >= ?1 AND ts_ms < ?2"
        .to_string();
    let mut args = vec![Sql::Integer(r.start_ms), Sql::Integer(r.end_ms)];
    if let Some(f) = file {
        sql.push_str(" AND file = ?3");
        args.push(Sql::Text(f.clone()));
    }
    sql.push_str(" GROUP BY file, script");
    if let Ok(mut q) = ctx.db.prepare(&sql) {
        if let Ok(rows) = q.query_map(params_from_iter(args), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        }) {
            for (f, s, n, l) in rows.flatten() {
                m.insert((f, s), (n as u64, l));
            }
        }
    }
    m
}

pub fn scripts(ctx: &Ctx, p: &Params) -> Scripts {
    let r = p.range(ctx.z, ctx.now);
    let ctl = ctx.ctl("/scripts", p, &r);
    let file = p.file_opt();
    let f = ctx.facts(&r, &Filter::file(&file));
    let marks = marks_by_script(ctx, &r, &file);
    let gs = group(&f, |x| (x.file.clone(), x.script.clone()));
    let mut keys: BTreeSet<(String, String)> = gs
        .iter()
        .map(|g| (g.file.clone(), g.name.clone()))
        .collect();
    keys.extend(marks.keys().cloned());
    let max = gs.iter().map(|g| g.freeze.total).max().unwrap_or(0);
    let src = match p.src.as_str() {
        "context" | "mark" | "busy" => p.src.clone(),
        _ => "all".into(),
    };
    let mut rows = Vec::new();
    let empty = Group::default();
    let mut order: Vec<&(String, String)> = keys.iter().collect();
    let by_key: HashMap<(String, String), &Group> = gs
        .iter()
        .map(|g| ((g.file.clone(), g.name.clone()), g))
        .collect();
    order.sort_by_key(|k| {
        let g = by_key.get(*k).copied().unwrap_or(&empty);
        (
            std::cmp::Reverse(g.freeze.total),
            std::cmp::Reverse(g.busy.total),
            (*k).clone(),
        )
    });
    for k in order {
        let g = by_key.get(k).copied().unwrap_or(&empty);
        if k.1.is_empty() {
            continue;
        }
        let mk = marks.get(k);
        let mut tags = vec![];
        if g.freeze.n > 0 {
            tags.push("context".to_string());
        }
        if mk.is_some() {
            tags.push("mark".to_string());
        }
        if g.busy.n > 0 {
            tags.push("script_busy".to_string());
        }
        let keep = match src.as_str() {
            "context" => g.freeze.n > 0,
            "mark" => mk.is_some(),
            "busy" => g.busy.n > 0,
            _ => true,
        };
        if !keep {
            continue;
        }
        let wl = g.worst_layout();
        rows.push(SRow {
            name: k.1.clone(),
            url: ctl.script_url(&k.0, &k.1),
            file: k.0.clone(),
            tags,
            n: num(g.freeze.n),
            n_v: g.freeze.n,
            total: dur(g.freeze.total),
            total_v: g.freeze.total,
            split: Split::new(&g.freeze, max),
            p50: if g.freeze.n > 0 {
                dur(g.freeze.p50())
            } else {
                "—".into()
            },
            p95: if g.freeze.n > 0 {
                dur(g.freeze.p95())
            } else {
                "—".into()
            },
            pct: if g.freeze.n > 0 {
                pct(g.freeze.share())
            } else {
                "—".into()
            },
            runs: num(g.busy.n),
            busy: if g.busy.n > 0 {
                dur(g.busy.total)
            } else {
                "—".into()
            },
            busy_v: g.busy.total,
            accounts: num(g.accounts.iter().filter(|a| !a.is_empty()).count() as u64),
            layout_url: if wl.is_empty() {
                String::new()
            } else {
                ctl.layout_url(&k.0, &wl)
            },
            layout: if wl.is_empty() { "—".into() } else { wl },
        });
    }
    let fz = Agg::of(freezes_only(&f));
    // "No script name" over all files.
    let unnamed: Vec<&Fact> = freezes_only(&f).filter(|x| x.script.is_empty()).collect();
    let ua = Agg::of(unnamed.iter().copied());
    let ul: BTreeSet<(&str, &str)> = unnamed
        .iter()
        .map(|x| (x.file.as_str(), x.layout.as_str()))
        .collect();
    let none = (ua.n > 0).then(|| NoScript {
        n: num(ua.n),
        total: dur(ua.total),
        pct: pct(ua.share()),
        layouts: num(ul.len() as u64),
        url: ctl.freezes_url(&format!("&script={NO_SCRIPT}")),
    });
    let named = Agg::of(freezes_only(&f).filter(|x| !x.script.is_empty()));
    let busy = Agg::of(f.iter().filter(|x| x.kind == Kind::Busy));
    let nscripts = gs.iter().filter(|g| !g.name.is_empty()).count();
    let tiles = vec![
        tile(
            "Named scripts",
            num(nscripts as u64),
            format!("{} with marks", marks.len()),
        ),
        tile(
            "Freezes with a script name",
            num(named.n),
            format!(
                "{} of {} freezes",
                pct(if fz.n == 0 {
                    0.0
                } else {
                    named.n as f64 / fz.n as f64
                }),
                num(fz.n)
            ),
        ),
        tile(
            "Script busy",
            num(busy.n),
            if busy.n > 0 {
                dur(busy.total)
            } else {
                "—".into()
            },
        ),
    ];
    let src_urls = [
        ("all", "All"),
        ("context", "Freeze context"),
        ("mark", "Mark"),
        ("busy", "script_busy"),
    ]
    .iter()
    .map(|(k, l)| {
        (
            format!("/scripts?{}&src={k}", ctl.qs()),
            l.to_string(),
            src == *k,
        )
    })
    .collect();
    Scripts {
        tiles,
        src,
        src_urls,
        rows,
        none,
        ctl,
    }
}

pub struct ScriptDetail {
    pub ctl: Ctl,
    pub name: String,
    pub file: String,
    pub tags: Vec<String>,
    pub tiles: Vec<Tile>,
    pub lanes: Option<Lanes>,
    pub chart_title: String,
    pub chart: Chart,
    pub by_dur: Hist,
    pub users: Vec<NRow>,
    pub layouts: Vec<NRow>,
    pub runs: Vec<FRow>,
    pub busy_runs: Vec<FRow>,
    pub no_name: bool,
}

pub fn script_detail(ctx: &Ctx, p: &Params) -> ScriptDetail {
    let r = p.range(ctx.z, ctx.now);
    let mut ctl = ctx.ctl("/script", p, &r);
    ctl.keep = vec![
        ("file".into(), p.file.clone()),
        ("name".into(), p.name.clone()),
    ];
    ctl.file.clear();
    ctl.show_file = false;
    let no_name = p.name == NO_SCRIPT;
    let script = if no_name {
        String::new()
    } else {
        p.name.clone()
    };
    let filt = Filter {
        file: Some(p.file.clone()),
        script: Some(script.clone()),
        ..Default::default()
    };
    let f = ctx.facts(&r, &filt);
    let fz: Vec<Fact> = freezes_only(&f).cloned().collect();
    let a = Agg::of(&fz);
    let busy = Agg::of(f.iter().filter(|x| x.kind == Kind::Busy));
    let cmp = compare(ctx, p, &r, &filt);
    let mut tiles = detail_tiles(&a, cmp.as_ref().map(|(r, a)| (r, a)), ctx, false);
    tiles[0].label = "Runs that froze".into();
    let marks = marks_by_script(ctx, &r, &Some(p.file.clone()));
    let mut tags = vec![format!("context {}", num(a.n))];
    if let Some((n, label)) = marks.get(&(p.file.clone(), script.clone())) {
        tags.push(format!("mark \"{label}\" {}", num(*n)));
    }
    tags.push(format!("script_busy {}", num(busy.n)));
    if no_name {
        tags = vec!["scripts=0".into()];
    }
    let (title, bars) = series(&f, ctx.z, &r, ctx.now);
    let users = group(&fz, |x| (String::new(), x.account.clone()));
    let layouts = group(&fz, |x| (x.file.clone(), x.layout.clone()));
    let q = FreezeQuery {
        start_ms: r.start_ms,
        end_ms: r.end_ms,
        filter: filt.clone(),
        limit: 12,
        ..Default::default()
    };
    let (runs, _) = freeze_rows(ctx, &ctl, &q);
    let (busy_runs, _) = freeze_rows(
        ctx,
        &ctl,
        &FreezeQuery {
            types: vec!["script_busy"],
            limit: 8,
            ..q.clone()
        },
    );
    ScriptDetail {
        name: if no_name {
            "No script name".into()
        } else {
            p.name.clone()
        },
        file: p.file.clone(),
        tags,
        tiles,
        lanes: {
            let base = format!("/script?file={}&name={}", enc(&p.file), enc(&p.name));
            day_lanes(
                ctx,
                p,
                &r,
                &f,
                &[],
                &|d| day_link(&base, d),
                format!(
                    "/freezes?{}&file={}&script={}",
                    ctl.rq(),
                    enc(&p.file),
                    enc(&p.name)
                ),
            )
        },
        chart_title: title.into(),
        chart: Chart::stack(&bars, 150),
        by_dur: Hist::new(&DUR_LABELS, &a.dur_buckets(), "runs"),
        users: nrows(&users, &ctl, "user", 8, ctx),
        layouts: nrows(&layouts, &ctl, "layout", 8, ctx),
        runs,
        busy_runs,
        no_name,
        ctl,
    }
}

// ---------------------------------------------------------------- users

pub struct URow {
    pub account: String,
    pub url: String,
    pub user: String,
    pub clients: String,
    pub client_url: String,
    pub seen: String,
    pub n: String,
    pub n_v: u64,
    pub total: String,
    pub total_v: u64,
    pub split: Split,
    pub pct: String,
    pub pct_v: u64,
    pub n7: String,
    pub total7: String,
    pub worst: String,
    pub worst_url: String,
}

pub struct Users {
    pub ctl: Ctl,
    pub tiles: Vec<Tile>,
    pub rows: Vec<URow>,
    pub foot: URow,
}

/// The newest non-empty user name of each account.
fn user_names(f: &[Fact]) -> HashMap<String, String> {
    let mut m: HashMap<String, (i64, String)> = HashMap::new();
    for x in f.iter().filter(|x| !x.user.is_empty()) {
        let e = m
            .entry(x.account.clone())
            .or_insert((i64::MIN, String::new()));
        if x.ts_ms >= e.0 {
            *e = (x.ts_ms, x.user.clone());
        }
    }
    m.into_iter().map(|(k, v)| (k, v.1)).collect()
}

pub fn users(ctx: &Ctx, p: &Params) -> Users {
    let today = Range::today(ctx.z, ctx.now);
    let week = Range::week(ctx.z, ctx.now);
    let mut ctl = ctx.ctl("/users", p, &week);
    ctl.show_range = false;
    ctl.range = "today".into();
    ctl.date = format!(
        "{} {}",
        ctx.z.fmt(ctx.now, "%a, %b %-d, %Y · %H:%M"),
        ctx.z.abbr(ctx.now)
    );
    let file = p.file_opt();
    let f7 = ctx.facts(&week, &Filter::file(&file));
    let names = user_names(&f7);
    let g7 = group(&f7, |x| (String::new(), x.account.clone()));
    let ft: Vec<Fact> = f7
        .iter()
        .filter(|x| x.ts_ms >= today.start_ms)
        .cloned()
        .collect();
    let gt: HashMap<String, Group> = group(&ft, |x| (String::new(), x.account.clone()))
        .into_iter()
        .map(|g| (g.name.clone(), g))
        .collect();
    let max = gt.values().map(|g| g.freeze.total).max().unwrap_or(0);
    let empty = Group::default();
    let mut rows: Vec<URow> = g7
        .iter()
        .map(|g| {
            let t = gt.get(&g.name).unwrap_or(&empty);
            let cl: Vec<i64> = g.clients.iter().copied().collect();
            let cname = match cl.len() {
                0 => "—".to_string(),
                1 => ctx.client_name(cl[0]),
                n => format!("{} +{}", ctx.client_name(cl[0]), n - 1),
            };
            let wl = g.worst_layout();
            let wfile = f7
                .iter()
                .find(|x| x.account == g.name && x.layout == wl)
                .map(|x| x.file.clone())
                .unwrap_or_default();
            let last = g.freeze.last_ms.max(g.busy.last_ms);
            URow {
                account: person(&g.name),
                url: ctl.user_url(&g.name),
                user: names.get(&g.name).cloned().unwrap_or_default(),
                clients: cname,
                client_url: cl.first().map(|c| ctl.client_url(*c)).unwrap_or_default(),
                seen: if last > 0 {
                    ctx.when(last)
                } else {
                    "—".into()
                },
                n: num(t.freeze.n),
                n_v: t.freeze.n,
                total: dur(t.freeze.total),
                total_v: t.freeze.total,
                split: Split::new(&t.freeze, max),
                pct: pct(t.freeze.share()),
                pct_v: (t.freeze.share() * 1000.0) as u64,
                n7: num(g.freeze.n),
                total7: dur(g.freeze.total),
                worst_url: if wl.is_empty() {
                    String::new()
                } else {
                    ctl.layout_url(&wfile, &wl)
                },
                worst: if wl.is_empty() { "—".into() } else { wl },
            }
        })
        .collect();
    rows.sort_by(|a, b| b.total_v.cmp(&a.total_v).then(a.account.cmp(&b.account)));
    let at = Agg::of(freezes_only(&ft));
    let a7 = Agg::of(freezes_only(&f7));
    let all_clients: BTreeSet<i64> = f7.iter().map(|x| x.client_id).collect();
    let foot = URow {
        account: "All".into(),
        url: String::new(),
        user: String::new(),
        clients: num(all_clients.len() as u64),
        client_url: String::new(),
        seen: String::new(),
        n: num(at.n),
        n_v: 0,
        total: dur(at.total),
        total_v: 0,
        split: Split::new(&at, at.total),
        pct: pct(at.share()),
        pct_v: 0,
        n7: num(a7.n),
        total7: dur(a7.total),
        worst: String::new(),
        worst_url: String::new(),
    };
    // Tiles.
    let by_client = group(&f7, |x| (String::new(), x.client_id.to_string()));
    let shared: Vec<&Group> = by_client
        .iter()
        .filter(|g| g.accounts.iter().filter(|a| !a.is_empty()).count() > 1)
        .collect();
    let multi: Vec<&Group> = g7.iter().filter(|g| g.clients.len() > 1).collect();
    let top = gt.values().max_by_key(|g| (g.freeze.total, g.name.clone()));
    let mut t2 = tile(
        "Most time frozen today",
        top.map(|g| person(&g.name)).unwrap_or_else(|| "—".into()),
        top.map(|g| format!("{} · {} freezes", dur(g.freeze.total), num(g.freeze.n)))
            .unwrap_or_default(),
    );
    if let Some(g) = top {
        t2.small = names.get(&g.name).cloned().unwrap_or_default();
        t2.link = ctl.user_url(&g.name);
    }
    let mut t3 = tile(
        "Shared clients",
        num(shared.len() as u64),
        shared
            .first()
            .map(|g| {
                let id: i64 = g.name.parse().unwrap_or(0);
                format!(
                    "{} · {}",
                    ctx.client_name(id),
                    g.accounts
                        .iter()
                        .filter(|a| !a.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .unwrap_or_else(|| "—".into()),
    );
    if let Some(g) = shared.first() {
        t3.link = ctl.client_url(g.name.parse().unwrap_or(0));
    }
    let mut t4 = tile(
        "Accounts on 2+ clients",
        num(multi.len() as u64),
        multi
            .first()
            .map(|g| format!("{} · 7 days", person(&g.name)))
            .unwrap_or_else(|| "—".into()),
    );
    if let Some(g) = multi.first() {
        t4.link = ctl.user_url(&g.name);
    }
    let tiles = vec![
        tile(
            "Accounts",
            num(g7.len() as u64),
            format!(
                "{} with freezes today",
                gt.values().filter(|g| g.freeze.n > 0).count()
            ),
        ),
        t2,
        t3,
        t4,
    ];
    Users {
        tiles,
        rows,
        foot,
        ctl,
    }
}

pub struct ClientCard {
    pub name: String,
    pub url: String,
    pub connected: bool,
    pub app: String,
    pub plugin: String,
    pub sessions: String,
    pub n: String,
    pub total: String,
}

pub struct UserDetail {
    pub ctl: Ctl,
    pub account: String,
    pub user: String,
    pub files: String,
    pub clients: Vec<ClientCard>,
    pub seen: String,
    pub connected: bool,
    pub tiles: Vec<Tile>,
    pub lanes: Option<Lanes>,
    pub chart_title: String,
    pub chart: Chart,
    pub layouts: Vec<NRow>,
    pub scripts: Vec<NRow>,
    pub busy: Vec<NRow>,
    pub by_dur: Hist,
    pub events: Vec<FRow>,
}

fn sessions_count(ctx: &Ctx, client: i64, since: i64) -> u64 {
    ctx.db
        .query_row(
            "SELECT count(*) FROM sessions WHERE client_id = ?1 AND last_seen_ms >= ?2",
            [client, since],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as u64
}

pub fn user_detail(ctx: &Ctx, p: &Params) -> UserDetail {
    let r = p.range(ctx.z, ctx.now);
    let mut ctl = ctx.ctl("/user", p, &r);
    ctl.keep = vec![("account".into(), p.account.clone())];
    ctl.file.clear();
    ctl.show_file = false;
    let filt = Filter {
        account: Some(p.account.clone()),
        ..Default::default()
    };
    let f = ctx.facts(&r, &filt);
    let fz: Vec<Fact> = freezes_only(&f).cloned().collect();
    let a = Agg::of(&fz);
    let busy = Agg::of(f.iter().filter(|x| x.kind == Kind::Busy));
    let cmp = compare(ctx, p, &r, &filt);
    let mut tiles = detail_tiles(&a, cmp.as_ref().map(|(r, a)| (r, a)), ctx, false);
    // The fifth tile is script busy for a user.
    let mut tb = tile(
        "Script busy",
        num(busy.n),
        if busy.n > 0 {
            dur(busy.total)
        } else {
            "—".into()
        },
    );
    if let Some((pr, _)) = &cmp {
        let pf = ctx.facts(pr, &filt);
        let pb = Agg::of(pf.iter().filter(|x| x.kind == Kind::Busy));
        tb.cmp = format!(
            "{} {} · {}",
            pr.short(),
            num(pb.n),
            fmt::signed(busy.n as i64 - pb.n as i64, "")
        );
    }
    tiles[4] = tb;
    // Week facts for the user name, clients and files.
    let wk = ctx.facts(&Range::week(ctx.z, ctx.now), &filt);
    let mut all = wk.clone();
    all.extend(f.iter().cloned());
    let user = user_names(&all)
        .get(&p.account)
        .cloned()
        .unwrap_or_default();
    let files: BTreeSet<String> = all
        .iter()
        .map(|x| x.file.clone())
        .filter(|x| !x.is_empty())
        .collect();
    let by_client = group(&all, |x| (String::new(), x.client_id.to_string()));
    let since = ctx.now - 7 * DAY_MS;
    let clients: Vec<ClientCard> = by_client
        .iter()
        .map(|g| {
            let id: i64 = g.name.parse().unwrap_or(0);
            let c = ctx.client(id).cloned().unwrap_or_default();
            ClientCard {
                name: ctx.client_name(id),
                url: ctl.client_url(id),
                connected: c.connected,
                app: c.app.clone(),
                plugin: c.plugin.clone(),
                sessions: num(sessions_count(ctx, id, since)),
                n: num(g.freeze.n),
                total: dur(g.freeze.total),
            }
        })
        .collect();
    let last = all.iter().map(|x| x.ts_ms).max();
    let (title, bars) = series(&f, ctx.z, &r, ctx.now);
    let layouts = group(&fz, |x| (x.file.clone(), x.layout.clone()));
    let scripts = group(&fz, |x| (x.file.clone(), x.script.clone()));
    let busy_rows: Vec<NRow> = group(
        &f.iter()
            .filter(|x| x.kind == Kind::Busy)
            .cloned()
            .collect::<Vec<_>>(),
        |x| (x.file.clone(), x.script.clone()),
    )
    .iter()
    .map(|g| NRow {
        name: script_label(&g.name),
        url: ctl.script_url(&g.file, &g.name),
        n: format!("{} runs", num(g.busy.n)),
        total: dur(g.busy.total),
        pct: pct(g.busy.share()),
        ..Default::default()
    })
    .collect();
    let (events, _) = freeze_rows(
        ctx,
        &ctl,
        &FreezeQuery {
            start_ms: r.start_ms,
            end_ms: r.end_ms,
            filter: filt.clone(),
            types: vec!["freeze", "script_busy", "mark"],
            limit: 12,
            ..Default::default()
        },
    );
    UserDetail {
        account: person(&p.account),
        user,
        files: files.into_iter().collect::<Vec<_>>().join(" · "),
        connected: clients.iter().any(|c| c.connected),
        clients,
        seen: last.map(|t| ctx.when(t)).unwrap_or_else(|| "—".into()),
        tiles,
        lanes: {
            let base = format!("/user?account={}", enc(&p.account));
            day_lanes(
                ctx,
                p,
                &r,
                &f,
                &[],
                &|d| day_link(&base, d),
                format!("/freezes?{}&account={}", ctl.rq(), enc(&p.account)),
            )
        },
        chart_title: title.into(),
        chart: Chart::stack(&bars, 160),
        layouts: nrows(&layouts, &ctl, "layout", 8, ctx),
        scripts: nrows(&scripts, &ctl, "script", 8, ctx),
        busy: busy_rows,
        by_dur: Hist::new(&DUR_LABELS, &a.dur_buckets(), "freezes"),
        events,
        ctl,
    }
}

// ---------------------------------------------------------------- clients

pub struct CRow {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub accounts: String,
    pub account_url: String,
    pub connected: bool,
    pub status: String,
    pub seen: String,
    pub quiet: String,
    pub app: String,
    pub plugin: String,
    pub old: bool,
    pub n: String,
    pub n_v: u64,
    pub total: String,
    pub total_v: u64,
    pub split: Split,
    pub pct: String,
    pub n7: String,
    pub total7: String,
    pub worst: String,
    pub worst_url: String,
}

pub struct Clients {
    pub ctl: Ctl,
    pub tiles: Vec<Tile>,
    pub app_versions: Vec<(String, String, bool)>,
    pub plugin_versions: Vec<(String, String, bool)>,
    pub flag_count: usize,
    pub flag_chips: Vec<String>,
    pub rows: Vec<CRow>,
    pub lanes: Lanes,
}

pub fn clients(ctx: &Ctx, p: &Params) -> Clients {
    let today = Range::today(ctx.z, ctx.now);
    let week = Range::week(ctx.z, ctx.now);
    let mut ctl = ctx.ctl("/clients", p, &week);
    ctl.show_range = false;
    ctl.range = "today".into();
    ctl.date = format!(
        "{} {}",
        ctx.z.fmt(ctx.now, "%a, %b %-d, %Y · %H:%M"),
        ctx.z.abbr(ctx.now)
    );
    let file = p.file_opt();
    let f7 = ctx.facts(&week, &Filter::file(&file));
    let g7: HashMap<i64, Group> = group(&f7, |x| (String::new(), x.client_id.to_string()))
        .into_iter()
        .map(|g| (g.name.parse().unwrap_or(0), g))
        .collect();
    let ft: Vec<Fact> = f7
        .iter()
        .filter(|x| x.ts_ms >= today.start_ms)
        .cloned()
        .collect();
    let gt: HashMap<i64, Group> = group(&ft, |x| (String::new(), x.client_id.to_string()))
        .into_iter()
        .map(|g| (g.name.parse().unwrap_or(0), g))
        .collect();
    let max = gt.values().map(|g| g.freeze.total).max().unwrap_or(0);
    let flags = query::flags(&ft, &ctx.clients, &ctx.s, ctx.now);
    let empty = Group::default();
    let mut rows: Vec<CRow> = ctx
        .clients
        .iter()
        .filter(|c| c.last_seen_ms >= ctx.now - 30 * DAY_MS || g7.contains_key(&c.id))
        .map(|c| {
            let t = gt.get(&c.id).unwrap_or(&empty);
            let g = g7.get(&c.id).unwrap_or(&empty);
            let accts: Vec<&String> = g.accounts.iter().filter(|a| !a.is_empty()).collect();
            let quiet = flags
                .quiet
                .iter()
                .find(|q| q.0 == c.id)
                .map(|q| quiet_text(q.2))
                .unwrap_or_default();
            let wl = g.worst_layout();
            let wfile = f7
                .iter()
                .find(|x| x.client_id == c.id && x.layout == wl)
                .map(|x| x.file.clone())
                .unwrap_or_default();
            CRow {
                id: c.id,
                name: c.label(),
                url: ctl.client_url(c.id),
                accounts: match accts.len() {
                    0 => "—".into(),
                    1 => accts[0].clone(),
                    n => format!("{} +{}", accts[0], n - 1),
                },
                account_url: accts.first().map(|a| ctl.user_url(a)).unwrap_or_default(),
                connected: c.connected,
                status: if c.connected {
                    "Connected".into()
                } else {
                    "Offline".into()
                },
                seen: ctx.when(c.last_seen_ms),
                quiet,
                app: c.app.clone(),
                old: !c.plugin.is_empty() && query::version_lt(&c.plugin, &ctx.s.flag_plugin_min),
                plugin: c.plugin.clone(),
                n: num(t.freeze.n),
                n_v: t.freeze.n,
                total: dur(t.freeze.total),
                total_v: t.freeze.total,
                split: Split::new(&t.freeze, max),
                pct: pct(t.freeze.share()),
                n7: num(g.freeze.n),
                total7: dur(g.freeze.total),
                worst_url: if wl.is_empty() {
                    String::new()
                } else {
                    ctl.layout_url(&wfile, &wl)
                },
                worst: if wl.is_empty() { "—".into() } else { wl },
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.total_v.cmp(&a.total_v))
            .then(a.name.cmp(&b.name))
    });
    let on = rows.iter().filter(|r| r.connected).count();
    let accounts_today: BTreeSet<&str> = ft
        .iter()
        .map(|x| x.account.as_str())
        .filter(|a| !a.is_empty())
        .collect();
    let files_today: BTreeSet<&str> = ft
        .iter()
        .map(|x| x.file.as_str())
        .filter(|a| !a.is_empty())
        .collect();
    let mut t1 = tile("Clients", num(rows.len() as u64), String::new());
    t1.sub = format!("{on} connected · {} offline", rows.len() - on);
    let mut t2 = tile(
        "Accounts today",
        num(accounts_today.len() as u64),
        files_today.iter().copied().collect::<Vec<_>>().join(" · "),
    );
    t2.link = "/users".into();
    let mut av: BTreeMap<String, usize> = BTreeMap::new();
    let mut pv: BTreeMap<String, usize> = BTreeMap::new();
    for r in &rows {
        *av.entry(if r.app.is_empty() {
            "—".into()
        } else {
            r.app.clone()
        })
        .or_default() += 1;
        *pv.entry(if r.plugin.is_empty() {
            "—".into()
        } else {
            r.plugin.clone()
        })
        .or_default() += 1;
    }
    let mut chips = vec![];
    if !flags.quiet.is_empty() {
        chips.push(format!("silent > {} min", ctx.s.flag_quiet_min));
    }
    if !flags.old.is_empty() {
        chips.push(format!("plug-in < {}", ctx.s.flag_plugin_min));
    }
    Clients {
        tiles: vec![t1, t2],
        app_versions: av
            .into_iter()
            .rev()
            .map(|(k, n)| (k, num(n as u64), false))
            .collect(),
        plugin_versions: pv
            .into_iter()
            .rev()
            .map(|(k, n)| {
                let old = query::version_lt(&k, &ctx.s.flag_plugin_min);
                (k, num(n as u64), old)
            })
            .collect(),
        flag_count: flags.quiet.len() + flags.old.len(),
        flag_chips: chips,
        rows,
        lanes: today_lanes(ctx, p),
        ctl,
    }
}

pub struct SessRow {
    pub session_id: String,
    pub client: i64,
    pub start: i64,
    pub end: i64,
    pub open: bool,
    pub freezes: u64,
    pub frozen_ms: u64,
    pub heartbeats: u64,
    pub app: String,
    pub plugin: String,
    pub options: String,
}

/// Sessions that overlap [a, b), optionally of one client.
pub fn sessions_between(ctx: &Ctx, a: i64, b: i64, client: Option<i64>) -> Vec<SessRow> {
    let mut sql = "SELECT session_id, client_id, coalesce(start_ms, first_seen_ms),
                          coalesce(stop_ms, last_seen_ms), stop_ms IS NULL, freezes, frozen_ms,
                          heartbeats, app, plugin, options
                   FROM sessions
                   WHERE coalesce(start_ms, first_seen_ms) < ?1 AND coalesce(stop_ms, last_seen_ms) >= ?2"
        .to_string();
    let mut args = vec![Sql::Integer(b), Sql::Integer(a)];
    if let Some(c) = client {
        sql.push_str(" AND client_id = ?3");
        args.push(Sql::Integer(c));
    }
    sql.push_str(" ORDER BY 3 DESC");
    let mut out = vec![];
    if let Ok(mut q) = ctx.db.prepare(&sql) {
        if let Ok(rows) = q.query_map(params_from_iter(args), |r| {
            Ok(SessRow {
                session_id: r.get(0)?,
                client: r.get(1)?,
                start: r.get(2)?,
                end: r.get(3)?,
                open: r.get(4)?,
                freezes: r.get::<_, i64>(5)? as u64,
                frozen_ms: r.get::<_, i64>(6)? as u64,
                heartbeats: r.get::<_, i64>(7)? as u64,
                app: r.get(8)?,
                plugin: r.get(9)?,
                options: r.get(10)?,
            })
        }) {
            out.extend(rows.flatten());
        }
    }
    out
}

pub struct SessView {
    pub day: String,
    pub start: String,
    pub stop: String,
    pub open: bool,
    pub n: String,
    pub total: String,
    pub heartbeats: String,
    pub app: String,
    pub plugin: String,
}

/// One hour of "SRTT by hour" on the Client page.
#[derive(Debug, Clone, Default)]
pub struct SrttRow {
    pub hour: String,
    pub n: String,
    pub p50: String,
    pub p95: String,
    /// Bar widths on a fixed scale (at least `SRTT_SCALE_MS`).
    pub w50: String,
    pub w95: String,
    pub title: String,
}

/// The smallest full scale of the SRTT bars: a LAN value of 1 ms stays a
/// short bar, and does not fill the bar as the largest value.
pub const SRTT_SCALE_MS: u64 = 50;

/// SRTT (the smoothed TCP round-trip time to FileMaker Server, from macOS)
/// of the freezes of each clock hour, newest hour first. A value of 0 means
/// "no reading" and is left out. Median and p95, nearest rank.
pub fn srtt_by_hour(ctx: &Ctx, fz: &[Fact]) -> Vec<SrttRow> {
    let mut by_h: BTreeMap<i64, Vec<u64>> = BTreeMap::new();
    for x in fz.iter().filter(|x| x.raw() && x.srtt > 0) {
        // The start of the clock hour (not the hour of the day, so two days
        // do not mix).
        let h = x.ts_ms - (x.ts_ms - ctx.z.day_start(x.day)).rem_euclid(crate::tz::HOUR_MS);
        by_h.entry(h).or_default().push(x.srtt);
    }
    let q = |v: &[u64], p: f64| {
        let rank = ((p * v.len() as f64).ceil() as usize).clamp(1, v.len());
        v[rank - 1]
    };
    let rows: Vec<(i64, usize, u64, u64)> = by_h
        .into_iter()
        .rev()
        .take(24)
        .map(|(h, mut v)| {
            v.sort_unstable();
            (h, v.len(), q(&v, 0.5), q(&v, 0.95))
        })
        .collect();
    let max = rows
        .iter()
        .map(|r| r.3)
        .max()
        .unwrap_or(0)
        .max(SRTT_SCALE_MS);
    rows.into_iter()
        .map(|(h, n, p50, p95)| {
            let hour = format!(
                "{}–{}",
                {
                    let t = ctx.when(h);
                    t.strip_suffix(":00").map(str::to_string).unwrap_or(t)
                },
                ctx.z.fmt(h + crate::tz::HOUR_MS, "%H:%M")
            );
            SrttRow {
                title: format!(
                    "{hour} {}: median {p50} ms, p95 {p95} ms, {n} freezes",
                    ctx.z.abbr(h)
                ),
                hour,
                n: num(n as u64),
                p50: format!("{p50}\u{a0}ms"),
                p95: format!("{p95}\u{a0}ms"),
                w50: w(p50, max),
                w95: w(p95, max),
            }
        })
        .collect()
}

pub struct ClientDetail {
    pub ctl: Ctl,
    pub found: bool,
    pub name: String,
    pub connected: bool,
    pub ip: String,
    pub app: String,
    pub plugin: String,
    pub os: String,
    pub machine: String,
    pub seen: String,
    pub tiles: Vec<Tile>,
    pub sessions: Vec<SessView>,
    pub options: Vec<(String, String)>,
    pub options_when: String,
    pub users: Vec<NRow>,
    pub layouts: Vec<NRow>,
    pub srtt: Vec<SrttRow>,
    pub lanes: Option<Lanes>,
    pub chart_title: String,
    pub chart: Chart,
    pub events: Vec<FRow>,
}

pub fn client_detail(ctx: &Ctx, p: &Params) -> ClientDetail {
    let r = p.range(ctx.z, ctx.now);
    let mut ctl = ctx.ctl("/client", p, &r);
    ctl.keep = vec![("id".into(), p.id.clone())];
    ctl.file.clear();
    ctl.show_file = false;
    let id: i64 = p.id.parse().unwrap_or(0);
    let c = ctx.client(id).cloned();
    let filt = Filter {
        client: Some(id),
        ..Default::default()
    };
    let f = ctx.facts(&r, &filt);
    let fz: Vec<Fact> = freezes_only(&f).cloned().collect();
    let a = Agg::of(&fz);
    let cmp = compare(ctx, p, &r, &filt);
    let mut tiles = detail_tiles(&a, cmp.as_ref().map(|(r, a)| (r, a)), ctx, false);
    tiles.truncate(3);
    let range_sess = sessions_between(ctx, r.start_ms, r.end_ms, Some(id));
    let hb: u64 = range_sess.iter().map(|s| s.heartbeats).sum();
    let mut t_srtt = tile(
        "SRTT median",
        format!("{} ms", a.srtt_p(0.5)),
        format!("p95 {} ms", a.srtt_p(0.95)),
    );
    let mut t_hb = tile("Heartbeats", fmt::big(hb), "idle+runloop".into());
    if let Some((pr, pa)) = &cmp {
        t_srtt.cmp = format!(
            "{} {} ms · {}",
            pr.short(),
            pa.srtt_p(0.5),
            fmt::signed(a.srtt_p(0.5) as i64 - pa.srtt_p(0.5) as i64, " ms")
        );
        let ph: u64 = sessions_between(ctx, pr.start_ms, pr.end_ms, Some(id))
            .iter()
            .map(|s| s.heartbeats)
            .sum();
        t_hb.cmp = format!(
            "{} {} · {}",
            pr.short(),
            fmt::big(ph),
            fmt::change(hb as f64, ph as f64)
        );
    }
    tiles.push(t_srtt);
    tiles.push(t_hb);
    let week_sess = sessions_between(ctx, ctx.now - 7 * DAY_MS, ctx.now + DAY_MS, Some(id));
    let sessions = week_sess
        .iter()
        .take(20)
        .map(|s| SessView {
            day: ctx.z.fmt(s.start, "%a, %b %-d"),
            start: ctx.z.hms(s.start),
            stop: if s.open {
                String::new()
            } else {
                ctx.z.hms(s.end)
            },
            open: s.open,
            n: num(s.freezes),
            total: dur(s.frozen_ms),
            heartbeats: num(s.heartbeats),
            app: s.app.trim_start_matches("Pro ").to_string(),
            plugin: s.plugin.clone(),
        })
        .collect();
    let latest = week_sess.iter().find(|s| !s.options.is_empty());
    let mut options = vec![];
    if let Some(s) = latest {
        if let Ok(serde_json::Value::Object(o)) =
            serde_json::from_str::<serde_json::Value>(&s.options)
        {
            for (k, v) in o {
                let v = match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                options.push((k, v));
            }
        }
    }
    let users = group(&fz, |x| (String::new(), x.account.clone()));
    let layouts = group(&fz, |x| (x.file.clone(), x.layout.clone()));
    let srtt = srtt_by_hour(ctx, &fz);
    let (title, bars) = series(&f, ctx.z, &r, ctx.now);
    let (events, _) = freeze_rows(
        ctx,
        &ctl,
        &FreezeQuery {
            start_ms: r.start_ms,
            end_ms: r.end_ms,
            filter: filt.clone(),
            types: vec!["freeze", "script_busy", "mark"],
            limit: 12,
            ..Default::default()
        },
    );
    let c2 = c.clone().unwrap_or_default();
    ClientDetail {
        found: c.is_some(),
        name: c
            .as_ref()
            .map(|c| c.label())
            .unwrap_or_else(|| format!("client {id}")),
        connected: c2.connected,
        ip: c2.ip.clone(),
        app: c2.app.clone(),
        plugin: c2.plugin.clone(),
        os: c2.os.clone(),
        machine: c2.machine.clone(),
        seen: if c2.last_seen_ms > 0 {
            ctx.when(c2.last_seen_ms)
        } else {
            "—".into()
        },
        tiles,
        sessions,
        options_when: latest.map(|s| ctx.when_tz(s.start)).unwrap_or_default(),
        options,
        users: nrows(&users, &ctl, "user", 8, ctx),
        layouts: nrows(&layouts, &ctl, "layout", 8, ctx),
        srtt,
        lanes: day_lanes(
            ctx,
            p,
            &r,
            &f,
            &range_sess,
            &|d| day_link(&format!("/client?id={id}"), d),
            format!("/freezes?{}&client={id}", ctl.rq()),
        ),
        chart_title: title.into(),
        chart: Chart::stack(&bars, 150),
        events,
        ctl,
    }
}

// ---------------------------------------------------------------- freezes

pub struct Opt {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

pub struct Freezes {
    pub ctl: Ctl,
    pub layouts: Vec<Opt>,
    pub accounts: Vec<Opt>,
    pub scripts: Vec<Opt>,
    pub clients: Vec<Opt>,
    pub layout: String,
    pub account: String,
    pub script: String,
    pub client: String,
    pub rows: Vec<FRow>,
    pub shown: u64,
    pub total: u64,
    pub more_url: String,
    pub filtered: bool,
}

pub const PAGE: usize = 100;

pub fn freeze_filter(p: &Params) -> Filter {
    let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
    Filter {
        file: p.file_opt(),
        layout: opt(&p.layout),
        script: if p.script == NO_SCRIPT {
            Some(String::new())
        } else {
            opt(&p.script)
        },
        account: if p.account.is_empty() {
            None
        } else if p.account == "~none" {
            Some(String::new())
        } else {
            Some(p.account.clone())
        },
        client: p.client.parse().ok(),
    }
}

fn freeze_qs(ctl: &Ctl, p: &Params) -> String {
    let mut s = ctl.qs();
    for (k, v) in [
        ("layout", &p.layout),
        ("account", &p.account),
        ("script", &p.script),
        ("client", &p.client),
    ] {
        if !v.is_empty() {
            s.push_str(&format!("&{k}={}", enc(v)));
        }
    }
    s
}

pub fn freezes(ctx: &Ctx, p: &Params) -> Freezes {
    let r = p.range(ctx.z, ctx.now);
    let ctl = ctx.ctl("/freezes", p, &r);
    let filt = freeze_filter(p);
    let before = p
        .before
        .split_once(':')
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)));
    let (rows, total) = freeze_rows(
        ctx,
        &ctl,
        &FreezeQuery {
            start_ms: r.start_ms,
            end_ms: r.end_ms,
            filter: filt.clone(),
            before,
            limit: PAGE,
            ..Default::default()
        },
    );
    // Options from the range (file filter only).
    let f = ctx.facts(&r, &Filter::file(&p.file_opt()));
    let fz: Vec<&Fact> = freezes_only(&f).filter(|x| x.raw()).collect();
    let mk = |vals: BTreeSet<String>, cur: &str, label: &dyn Fn(&str) -> String| -> Vec<Opt> {
        vals.into_iter()
            .map(|v| Opt {
                selected: v == cur,
                label: label(&v),
                value: v,
            })
            .collect()
    };
    let layouts = mk(
        fz.iter()
            .map(|x| x.layout.clone())
            .filter(|x| !x.is_empty())
            .collect(),
        &p.layout,
        &|v| v.to_string(),
    );
    let accounts = mk(
        fz.iter()
            .map(|x| {
                if x.account.is_empty() {
                    "~none".to_string()
                } else {
                    x.account.clone()
                }
            })
            .collect(),
        &p.account,
        &|v| {
            if v == "~none" {
                "—".into()
            } else {
                v.to_string()
            }
        },
    );
    let scripts = mk(
        fz.iter()
            .map(|x| {
                if x.script.is_empty() {
                    NO_SCRIPT.to_string()
                } else {
                    x.script.clone()
                }
            })
            .collect(),
        &p.script,
        &|v| {
            if v == NO_SCRIPT {
                "No script name".into()
            } else {
                v.to_string()
            }
        },
    );
    let ids: BTreeSet<String> = fz.iter().map(|x| x.client_id.to_string()).collect();
    let mut clients: Vec<Opt> = ids
        .into_iter()
        .map(|v| Opt {
            selected: v == p.client,
            label: ctx.client_name(v.parse().unwrap_or(0)),
            value: v,
        })
        .collect();
    clients.sort_by(|a, b| a.label.cmp(&b.label));
    let more_url = rows
        .last()
        .filter(|_| rows.len() == PAGE)
        .map(|l| {
            format!(
                "/freezes/more?{}&before={}:{}&shown={}",
                freeze_qs(&ctl, p),
                l.ts_ms,
                l.id,
                p.shown + rows.len() as u64
            )
        })
        .unwrap_or_default();
    Freezes {
        shown: p.shown + rows.len() as u64,
        total,
        more_url,
        filtered: !(p.layout.is_empty()
            && p.account.is_empty()
            && p.script.is_empty()
            && p.client.is_empty()
            && p.file_opt().is_none()),
        layout: p.layout.clone(),
        account: p.account.clone(),
        script: p.script.clone(),
        client: p.client.clone(),
        layouts,
        accounts,
        scripts,
        clients,
        rows,
        ctl,
    }
}

pub struct CtxRow {
    pub field: String,
    pub before: String,
    pub after: String,
    pub changed: bool,
    pub link_before: String,
    pub link_after: String,
}

pub struct MarkRow {
    pub t: String,
    pub off: String,
    pub label: String,
}

pub struct FreezeDetail {
    pub ctl: Ctl,
    pub found: bool,
    pub id: i64,
    pub crumb: String,
    pub from: String,
    pub from_url: String,
    pub layout: String,
    pub layout_url: String,
    pub when: String,
    pub client: String,
    pub client_url: String,
    pub account: String,
    pub account_url: String,
    pub user: String,
    pub script: String,
    pub script_url: String,
    pub file: String,
    pub level: String,
    pub prev_url: String,
    pub next_url: String,
    pub tiles: Vec<Tile>,
    pub srv_label: String,
    pub cli_label: String,
    pub axis: Vec<(String, String)>,
    pub span_x: String,
    pub span_w: String,
    pub span_srv: String,
    pub span_end: String,
    pub t0: String,
    pub t1: String,
    pub tl_marks: Vec<(String, String, String)>,
    pub context: Vec<CtxRow>,
    pub connected: bool,
    pub net: Vec<(String, String, String)>,
    pub marks: Vec<MarkRow>,
    pub strip: Option<Lanes>,
    pub around: Vec<FRow>,
}

pub fn freeze_detail(ctx: &Ctx, p: &Params) -> FreezeDetail {
    let id: i64 = p.id.parse().unwrap_or(0);
    let r = Range::week(ctx.z, ctx.now);
    let mut ctl = ctx.ctl("/freeze", p, &r);
    ctl.show_file = false;
    ctl.show_range = false;
    let row = ctx
        .db
        .query_row(
            "SELECT e.ts_ms, e.end_ms, e.duration_ms, e.wait_ms, e.round_trips, e.srtt_ms,
                    e.tx_bytes, e.rx_bytes, e.tx_packets, e.rx_packets, e.connected, e.level,
                    e.file, e.layout, e.from_layout, e.script, e.account, e.user, e.client_id,
                    e.marks, e.before_json, e.after_json, e.session_id, s.port
             FROM events e LEFT JOIN sessions s ON s.session_id = e.session_id
             WHERE e.id = ?1 AND e.type = 'freeze'",
            [id],
            |r| {
                Ok((
                    [
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get::<_, i64>(18)?,
                        r.get::<_, Option<i64>>(23)?.unwrap_or(0),
                    ],
                    [
                        r.get::<_, String>(11)?,
                        r.get(12)?,
                        r.get(13)?,
                        r.get(14)?,
                        r.get(15)?,
                        r.get(16)?,
                        r.get(17)?,
                        r.get(19)?,
                        r.get(20)?,
                        r.get(21)?,
                        r.get(22)?,
                    ],
                ))
            },
        )
        .optional()
        .ok()
        .flatten();
    let Some((n, s)) = row else {
        return FreezeDetail {
            ctl,
            found: false,
            id,
            crumb: String::new(),
            from: String::new(),
            from_url: String::new(),
            layout: String::new(),
            layout_url: String::new(),
            when: String::new(),
            client: String::new(),
            client_url: String::new(),
            account: String::new(),
            account_url: String::new(),
            user: String::new(),
            script: String::new(),
            script_url: String::new(),
            file: String::new(),
            level: String::new(),
            prev_url: String::new(),
            next_url: String::new(),
            tiles: vec![],
            srv_label: String::new(),
            cli_label: String::new(),
            axis: vec![],
            span_x: String::new(),
            span_w: String::new(),
            span_srv: String::new(),
            span_end: String::new(),
            t0: String::new(),
            t1: String::new(),
            tl_marks: vec![],
            context: vec![],
            connected: false,
            net: vec![],
            marks: vec![],
            strip: None,
            around: vec![],
        };
    };
    let [ts, end, d, wt, rt, srtt, txb, rxb, txp, rxp, conn, cid, port] = n;
    let [level, file, layout, from, script, account, user, marks_json, before_json, after_json, session_id] =
        s;
    let end = if end > 0 { end } else { ts + d };
    let d = d.max(0) as u64;
    let wt = (wt.max(0) as u64).min(d);
    let cl = d - wt;
    // The layout p95 over 7 days, for the duration tile.
    let lf = ctx.facts(
        &r,
        &Filter {
            file: Some(file.clone()),
            layout: Some(layout.clone()),
            ..Default::default()
        },
    );
    let lp95 = Agg::of(freezes_only(&lf)).p95();
    let tiles = vec![
        tile("Duration", dur1(d), format!("p95 for layout {}", dur(lp95))),
        tile(
            "Server wait",
            dur1(wt),
            format!(
                "{} of freeze",
                pct(if d == 0 { 0.0 } else { wt as f64 / d as f64 })
            ),
        ),
        tile(
            "Client time",
            dur1(cl),
            format!(
                "{} of freeze",
                pct(if d == 0 { 0.0 } else { cl as f64 / d as f64 })
            ),
        ),
        tile(
            "Round trips",
            num(rt.max(0) as u64),
            if rt > 0 {
                format!("{:.1} ms wait each", wt as f64 / rt as f64)
            } else {
                "—".into()
            },
        ),
    ];
    // Marks: the labels of the freeze, with their times from the mark events.
    let labels: Vec<String> = serde_json::from_str(&marks_json).unwrap_or_default();
    let mut marks = vec![];
    if !labels.is_empty() {
        if let Ok(mut q) = ctx.db.prepare(
            "SELECT ts_ms, label FROM events WHERE session_id = ?1 AND type = 'mark'
               AND ts_ms >= ?2 AND ts_ms <= ?3 ORDER BY ts_ms",
        ) {
            if let Ok(rows) = q.query_map(rusqlite::params![session_id, ts - 5000, end], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            }) {
                for (t, l) in rows.flatten() {
                    if labels.contains(&l) {
                        marks.push((t, l));
                    }
                }
            }
        }
        for l in &labels {
            if !marks.iter().any(|(_, m)| m == l) {
                marks.push((ts, l.clone()));
            }
        }
    }
    // Timeline: from the first mark (or a bit before) to a bit after the end.
    let first = marks.iter().map(|m| m.0).min().unwrap_or(ts).min(ts);
    let pad = ((end - first) / 8).max(100);
    let (a0, a1) = (first - pad, end + pad);
    let span = (a1 - a0).max(1) as f64;
    let x = |t: i64| (t - a0) as f64 / span * 100.0;
    let step = nice_step(a1 - a0);
    let mut axis = vec![];
    let mut t = (a0 / step + 1) * step;
    while t < a1 {
        let xt = x(t);
        if (6.0..=94.0).contains(&xt) {
            let label = if step % 1000 == 0 {
                ctx.z.fmt(t, "%H:%M:%S")
            } else {
                format!("{}.{:03}", ctx.z.fmt(t, "%H:%M:%S"), t.rem_euclid(1000))
            };
            axis.push((format!("{xt:.2}%"), label));
        }
        t += step;
    }
    let ms = |t: i64| format!("{}.{:03}", ctx.z.fmt(t, "%H:%M:%S"), t.rem_euclid(1000));
    let tl_marks = marks
        .iter()
        .enumerate()
        .map(|(i, (t, l))| {
            (
                format!("{:.2}%", x(*t)),
                if i % 2 == 0 { "0px" } else { "16px" }.to_string(),
                l.clone(),
            )
        })
        .collect();
    let mark_rows = marks
        .iter()
        .map(|(t, l)| MarkRow {
            t: ms(*t),
            off: format!(
                "{}{:.2} s",
                if *t < ts { "\u{2212}" } else { "+" },
                (ts - t).abs() as f64 / 1000.0
            ),
            label: l.clone(),
        })
        .collect();
    // Context before and after.
    let b: serde_json::Value = serde_json::from_str(&before_json).unwrap_or_default();
    let a: serde_json::Value = serde_json::from_str(&after_json).unwrap_or_default();
    let mut context = vec![];
    for k in [
        "file",
        "layout",
        "layout_table",
        "script",
        "account",
        "user",
        "window",
        "host",
        "mode",
    ] {
        let bv = b.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let av = a.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        if bv.is_empty() && av.is_empty() {
            continue;
        }
        let bf = b
            .get("file")
            .and_then(|v| v.as_str())
            .unwrap_or(&file)
            .to_string();
        let af = a
            .get("file")
            .and_then(|v| v.as_str())
            .unwrap_or(&file)
            .to_string();
        context.push(CtxRow {
            field: k.into(),
            changed: !a.is_null() && bv != av,
            link_before: if k == "layout" && !bv.is_empty() {
                ctl.layout_url(&bf, &bv)
            } else {
                String::new()
            },
            link_after: if k == "layout" && !av.is_empty() {
                ctl.layout_url(&af, &av)
            } else {
                String::new()
            },
            before: if bv.is_empty() { "—".into() } else { bv },
            after: if a.is_null() {
                "not read".into()
            } else if av.is_empty() {
                "—".into()
            } else {
                av
            },
        });
    }
    let net = vec![
        ("Round trips".into(), num(rt.max(0) as u64), String::new()),
        (
            "Wait".into(),
            format!("{} ms", num(wt)),
            if wt < d || (wt == d && d == 0) {
                String::new()
            } else {
                "≤ duration".into()
            },
        ),
        ("SRTT".into(), format!("{srtt} ms"), String::new()),
        (
            "Sent".into(),
            fmt::bytes(txb.max(0) as u64),
            format!("{} packets", num(txp.max(0) as u64)),
        ),
        (
            "Received".into(),
            fmt::bytes(rxb.max(0) as u64),
            format!("{} packets", num(rxp.max(0) as u64)),
        ),
        (
            "Port".into(),
            if port > 0 {
                port.to_string()
            } else {
                "—".into()
            },
            String::new(),
        ),
    ];
    // The same client's events around this freeze.
    let mut around = vec![];
    for (op, ord) in [("<", "DESC"), (">", "ASC")] {
        let sql = format!(
            "SELECT id, ts_ms FROM events WHERE client_id = ?1 AND type IN ('freeze','script_busy','mark')
               AND (ts_ms {op} ?2 OR (ts_ms = ?2 AND id {op} ?3)) ORDER BY ts_ms {ord}, id {ord} LIMIT 5"
        );
        if let Ok(mut q) = ctx.db.prepare(&sql) {
            if let Ok(rows) = q.query_map(rusqlite::params![cid, ts, id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            }) {
                around.extend(rows.flatten());
            }
        }
    }
    around.push((id, ts));
    around.sort_by_key(|x| (x.1, x.0));
    let lo = around.first().map(|x| x.1).unwrap_or(ts);
    let hi = around.last().map(|x| x.1).unwrap_or(ts) + 1;
    let (mut arows, _) = freeze_rows(
        ctx,
        &ctl,
        &FreezeQuery {
            start_ms: lo,
            end_ms: hi,
            filter: Filter {
                client: Some(cid),
                ..Default::default()
            },
            types: vec!["freeze", "script_busy", "mark"],
            limit: 40,
            ..Default::default()
        },
    );
    arows.reverse();
    // Previous and next freeze (any client).
    let nb = |op: &str, ord: &str| -> String {
        ctx.db
            .query_row(
                &format!(
                    "SELECT id FROM events WHERE type = 'freeze' AND (ts_ms {op} ?1 OR (ts_ms = ?1 AND id {op} ?2))
                     ORDER BY ts_ms {ord}, id {ord} LIMIT 1"
                ),
                [ts, id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .ok()
            .flatten()
            .map(|i| ctl.freeze_url(i))
            .unwrap_or_default()
    };
    let srv_pct = if d == 0 {
        0.0
    } else {
        wt as f64 / d as f64 * 100.0
    };
    let strip = {
        let (a, b) = (ts - 15 * 60_000, ts + 15 * 60_000);
        let sr = Range::custom(ctx.z, ctx.z.date_of(a), ctx.z.date_of(b));
        let sf = ctx.facts(
            &sr,
            &Filter {
                client: Some(cid),
                ..Default::default()
            },
        );
        Some(lanes::strip(ctx, cid, ts, id, &sf))
    };
    FreezeDetail {
        found: true,
        id,
        crumb: ms(ts),
        from_url: if from.is_empty() {
            String::new()
        } else {
            ctl.layout_url(&file, &from)
        },
        from: if from.is_empty() || from == layout {
            String::new()
        } else {
            from
        },
        layout_url: ctl.layout_url(&file, &layout),
        layout: if layout.is_empty() {
            "—".into()
        } else {
            layout
        },
        when: format!(
            "{} – {} {}",
            ctx.z.fmt(ts, "%a, %b %-d, %Y · ") + &ms(ts),
            ms(end),
            ctx.z.abbr(end)
        ),
        client: ctx.client_name(cid),
        client_url: ctl.client_url(cid),
        account_url: ctl.user_url(&account),
        account: person(&account),
        user,
        script_url: ctl.script_url(&file, &script),
        script: script_label(&script),
        file,
        level,
        prev_url: nb("<", "DESC"),
        next_url: nb(">", "ASC"),
        tiles,
        srv_label: format!("Server wait {}", dur1(wt)),
        cli_label: format!("Client time {}", dur1(cl)),
        axis,
        span_x: format!("{:.2}%", x(ts)),
        span_w: format!("{:.2}%", (x(end) - x(ts)).max(0.5)),
        span_srv: format!("{srv_pct:.1}%"),
        span_end: format!("{:.2}%", x(end)),
        t0: ms(ts),
        t1: ms(end),
        tl_marks,
        context,
        connected: conn != 0,
        net,
        marks: mark_rows,
        strip,
        around: arows,
        ctl,
    }
}

fn nice_step(span: i64) -> i64 {
    for s in [100, 200, 250, 500, 1000, 2000, 5000, 10_000, 30_000, 60_000] {
        if span / s <= 10 {
            return s;
        }
    }
    300_000
}

// ---------------------------------------------------------------- setup

pub struct Setup {
    pub ctl: Ctl,
    pub url: String,
    pub listen: String,
    pub since: String,
    pub token: String,
    pub show: bool,
    pub s: Settings,
    pub p95_s: String,
    pub db_path: String,
    pub events: String,
    pub totals: String,
    pub files: String,
    pub flag_p95: usize,
    pub flag_quiet: usize,
    pub flag_old: usize,
    pub clients: String,
    pub last_checkin: String,
    pub saved: bool,
}

impl Setup {
    /// The calculation for the plug-in, on one line.
    pub fn start_calc(&self) -> String {
        format!(
            "FlightRecorder_Start ( \"endpoint={}; token={}; threshold={}; port={}; send_ms={}; ping_ms={}\" )",
            self.url,
            self.token,
            self.s.rec_threshold_ms,
            self.s.rec_port,
            self.s.rec_send_ms,
            self.s.rec_ping_ms
        )
    }
    pub fn ping_s(&self) -> String {
        let s = self.s.rec_ping_ms as f64 / 1000.0;
        if s.fract() == 0.0 {
            format!("{s:.0}")
        } else {
            format!("{s}")
        }
    }
    pub fn masked(&self) -> String {
        "•".repeat(self.token.len().min(32))
    }
}

pub fn setup(ctx: &Ctx, p: &Params, saved: bool) -> Setup {
    let r = Range::week(ctx.z, ctx.now);
    let mut ctl = ctx.ctl("/setup", p, &r);
    ctl.show_file = false;
    ctl.show_range = false;
    let (ev_n, ev_bytes, ev_min): (i64, i64, Option<i64>) = ctx
        .db
        .query_row(
            "SELECT count(*), coalesce(sum(length(raw)), 0) + count(*) * 120, min(ts_ms) FROM events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap_or((0, 0, None));
    let (td_n, td_bytes, td_min): (i64, i64, Option<String>) = ctx
        .db
        .query_row(
            "SELECT count(DISTINCT day), coalesce(sum(length(hist)), 0) + count(*) * 100, min(day) FROM daily_totals",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap_or((0, 0, None));
    let flags = ctx.flags(&r, &None);
    let last = ctx.clients.iter().max_by_key(|c| c.last_seen_ms);
    Setup {
        url: ctx.app.public_url(),
        listen: ctx.app.listen_text(),
        since: format!("since {}", ctx.when(ctx.app.started_ms)),
        token: ctx.app.token(),
        show: p.show,
        p95_s: format!("{:.1}", ctx.s.flag_p95_ms as f64 / 1000.0),
        s: ctx.s.clone(),
        db_path: ctx.app.cfg.db.display().to_string(),
        events: format!(
            "{} · {} · {}",
            num(ev_n as u64),
            fmt::bytes(ev_bytes as u64),
            ev_min
                .map(|t| format!("since {}", ctx.z.fmt(t, "%b %-d, %Y")))
                .unwrap_or_else(|| "—".into())
        ),
        totals: format!(
            "{} days · {} · {}",
            num(td_n as u64),
            fmt::bytes(td_bytes as u64),
            td_min
                .and_then(|d| d.parse::<NaiveDate>().ok())
                .map(|d| format!("since {}", d.format("%b %-d, %Y")))
                .unwrap_or_else(|| "—".into())
        ),
        files: {
            let f = ctx.files();
            if f.is_empty() {
                "—".into()
            } else {
                f.join(" · ")
            }
        },
        flag_p95: flags.p95.len(),
        flag_quiet: flags.quiet.len(),
        flag_old: flags.old.len(),
        clients: num(ctx.clients.len() as u64),
        last_checkin: last
            .map(|c| format!("{} · {}", ctx.when(c.last_seen_ms), c.label()))
            .unwrap_or_else(|| "—".into()),
        saved,
        ctl,
    }
}

/// Parse and check the Setup form. Returns the new settings or an error.
pub fn parse_setup(form: &HashMap<String, String>, old: &Settings) -> Result<Settings, String> {
    let g = |k: &str| {
        form.get(k)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let mut s = old.clone();
    let num_in = |k: &str, lo: f64, hi: f64| -> Result<f64, String> {
        let v: f64 = g(k).parse().map_err(|_| format!("{k}: not a number"))?;
        if v < lo || v > hi {
            return Err(format!("{k}: from {lo} to {hi}"));
        }
        Ok(v)
    };
    s.flag_p95_ms = (num_in("flag_p95_s", 0.1, 600.0)? * 1000.0).round() as u64;
    s.flag_quiet_min = num_in("flag_quiet_min", 1.0, 100_000.0)? as u64;
    let v = g("flag_plugin_min");
    if v.is_empty() || !v.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err("flag_plugin_min: a version, for example 0.1.0".into());
    }
    s.flag_plugin_min = v;
    s.rec_threshold_ms = num_in("rec_threshold_ms", 20.0, 600_000.0)? as u64;
    s.rec_port = num_in("rec_port", 0.0, 65535.0)? as u16;
    // Optional in the form (an older page has no such fields).
    if !g("rec_send_ms").is_empty() {
        s.rec_send_ms = num_in("rec_send_ms", 250.0, 60_000.0)? as u64;
    }
    if !g("rec_ping_s").is_empty() {
        s.rec_ping_ms = (num_in("rec_ping_s", 5.0, 600.0)? * 1000.0).round() as u64;
    }
    s.event_days = num_in("event_days", 1.0, 3650.0)? as u32;
    s.total_days = num_in("total_days", 1.0, 36500.0)? as u32;
    Ok(s)
}
