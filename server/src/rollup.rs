//! Daily totals and retention.
//!
//! - Raw events are kept `event_days` days (default 30); daily totals are
//!   kept `total_days` days (default 730). Both are whole local days in the
//!   server zone.
//! - Each day that got new freeze or script_busy events is "dirty". When a
//!   dirty day has ended, its totals are written again from its raw events.
//! - Then the raw events before the cutoff day are deleted. A day is always
//!   rolled up before its events go, so its totals are complete.
//! - Percentiles cannot be added up, so each totals row keeps a histogram of
//!   the durations (see `query::BUCKETS_MS`). A percentile over totals is the
//!   upper edge of the bucket that holds it.

use crate::db::{Db, Settings};
use crate::query::{bucket_of, NB};
use crate::tz::Zone;
use chrono::{Days, NaiveDate};
use rusqlite::params;
use std::collections::BTreeMap;

/// The first day that keeps its raw events: today minus `event_days - 1`.
pub fn cutoff_day(zone: Zone, event_days: u32, now: i64) -> NaiveDate {
    let today = zone.date_of(now);
    today
        .checked_sub_days(Days::new(event_days.saturating_sub(1) as u64))
        .unwrap_or(today)
}

/// Close the sessions with no session_stop whose client went offline.
pub fn close_offline_sessions(db: &Db, now: i64, connected_ms: i64) -> rusqlite::Result<usize> {
    db.execute(
        "UPDATE sessions SET stop_ms = last_seen_ms, closed = 'offline'
         WHERE stop_ms IS NULL AND last_seen_ms < ?1",
        [now - connected_ms],
    )
}

/// An open freeze with no update for this long is over (the plug-in sends
/// one about each second while the freeze lasts).
pub const OPEN_STALE_MS: i64 = 60_000;

/// Close the open freezes (`freeze_progress` rows) whose session ended or
/// that had no update for [`OPEN_STALE_MS`]. They keep their last duration.
pub fn close_open_freezes(db: &Db, now: i64) -> rusqlite::Result<usize> {
    db.execute(
        "UPDATE events SET open = 0
         WHERE open = 1 AND (coalesce(end_ms, ts_ms) < ?1
           OR session_id IN (SELECT session_id FROM sessions WHERE closed <> ''))",
        [now - OPEN_STALE_MS],
    )
}

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub days_rolled: usize,
    pub events_deleted: usize,
    pub totals_deleted: usize,
    pub sessions_deleted: usize,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rolled up {} days; deleted {} events, {} total rows, {} sessions",
            self.days_rolled, self.events_deleted, self.totals_deleted, self.sessions_deleted
        )
    }
}

#[derive(Default)]
struct Tot {
    user: String,
    table: String,
    n: i64,
    total: i64,
    wait: i64,
    max: i64,
    rt_sum: i64,
    rt_max: i64,
    hist: [i64; NB],
}

/// Write the daily totals of one day from its raw events.
pub fn rollup_day(db: &mut Db, zone: Zone, day: NaiveDate) -> rusqlite::Result<usize> {
    let (s, e) = (zone.day_start(day), zone.day_end(day));
    let tx = db.transaction()?;
    let mut groups: BTreeMap<(String, String, String, String, String, i64), Tot> = BTreeMap::new();
    {
        let mut q = tx.prepare(
            "SELECT type, file, layout, layout_table, script, account, user, client_id,
                    duration_ms, wait_ms, round_trips
             FROM events
             WHERE ts_ms >= ?1 AND ts_ms < ?2 AND type IN ('freeze', 'script_busy')
             ORDER BY ts_ms",
        )?;
        let mut rows = q.query(params![s, e])?;
        while let Some(r) = rows.next()? {
            let key = (
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(7)?,
            );
            let t = groups.entry(key).or_default();
            let user: String = r.get(6)?;
            if !user.is_empty() {
                t.user = user;
            }
            let table: String = r.get(3)?;
            if !table.is_empty() {
                t.table = table;
            }
            let d: i64 = r.get(8)?;
            let w: i64 = r.get(9)?;
            let rt: i64 = r.get(10)?;
            t.n += 1;
            t.total += d;
            t.wait += w.min(d);
            t.max = t.max.max(d);
            t.rt_sum += rt;
            t.rt_max = t.rt_max.max(rt);
            t.hist[bucket_of(d as u64)] += 1;
        }
    }
    let ds = day.to_string();
    tx.execute("DELETE FROM daily_totals WHERE day = ?1", [&ds])?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO daily_totals(day, kind, file, layout, layout_table, script, account,
               user, client_id, n, total_ms, wait_ms, max_ms, rt_sum, rt_max, hist)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        )?;
        for ((kind, file, layout, script, account, client), t) in &groups {
            ins.execute(params![
                ds,
                kind,
                file,
                layout,
                t.table,
                script,
                account,
                t.user,
                client,
                t.n,
                t.total,
                t.wait,
                t.max,
                t.rt_sum,
                t.rt_max,
                serde_json::to_string(&t.hist.to_vec()).unwrap()
            ])?;
        }
    }
    tx.execute("DELETE FROM rollup_dirty WHERE day = ?1", [&ds])?;
    tx.commit()?;
    Ok(groups.len())
}

/// Roll up the dirty days that have ended, then apply the retention.
pub fn run(db: &mut Db, zone: Zone, s: &Settings, now: i64) -> rusqlite::Result<Report> {
    let today = zone.date_of(now);
    let mut rep = Report::default();
    let dirty: Vec<String> = {
        let mut q = db.prepare("SELECT day FROM rollup_dirty ORDER BY day")?;
        let v = q
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        v
    };
    for d in dirty {
        match d.parse::<NaiveDate>() {
            Ok(day) if day < today => {
                rollup_day(db, zone, day)?;
                rep.days_rolled += 1;
            }
            Ok(_) => {}
            Err(_) => {
                db.execute("DELETE FROM rollup_dirty WHERE day = ?1", [&d])?;
            }
        }
    }
    let cutoff = cutoff_day(zone, s.event_days, now);
    let cutoff_ms = zone.day_start(cutoff);
    rep.events_deleted = db.execute("DELETE FROM events WHERE ts_ms < ?1", [cutoff_ms])?;
    rep.sessions_deleted = db.execute(
        "DELETE FROM sessions WHERE last_seen_ms < ?1
           AND session_id NOT IN (SELECT DISTINCT session_id FROM events)",
        [cutoff_ms],
    )?;
    let oldest_total = today
        .checked_sub_days(Days::new(s.total_days.saturating_sub(1) as u64))
        .unwrap_or(today);
    rep.totals_deleted = db.execute(
        "DELETE FROM daily_totals WHERE day < ?1",
        [oldest_total.to_string()],
    )?;
    Ok(rep)
}
