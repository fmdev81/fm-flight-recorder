//! SQLite database: open, versioned migrations, settings and the Event token.
//!
//! The schema version is `PRAGMA user_version`. Each migration runs once,
//! in order, in a transaction, when the server opens the database.

use crate::config::SettingDefaults;
use rusqlite::{params, Connection, OptionalExtension};
use std::io::Read;
use std::path::Path;

pub type Db = Connection;

/// Each item is one schema version. Add new versions at the end; never edit
/// a version that has shipped.
const MIGRATIONS: &[&str] = &[
    // 1: the first schema.
    r#"
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    CREATE TABLE clients (
        id            INTEGER PRIMARY KEY,
        install_id    TEXT NOT NULL UNIQUE,
        name          TEXT NOT NULL DEFAULT '',
        machine       TEXT NOT NULL DEFAULT '',
        os            TEXT NOT NULL DEFAULT '',
        app           TEXT NOT NULL DEFAULT '',
        plugin        TEXT NOT NULL DEFAULT '',
        ip            TEXT NOT NULL DEFAULT '',
        first_seen_ms INTEGER NOT NULL,
        last_seen_ms  INTEGER NOT NULL
    );

    CREATE TABLE sessions (
        session_id    TEXT PRIMARY KEY,
        client_id     INTEGER NOT NULL REFERENCES clients(id),
        start_ms      INTEGER,
        stop_ms       INTEGER,
        closed        TEXT NOT NULL DEFAULT '',   -- '', 'stop' or 'offline'
        first_seen_ms INTEGER NOT NULL,
        last_seen_ms  INTEGER NOT NULL,
        app           TEXT NOT NULL DEFAULT '',
        plugin        TEXT NOT NULL DEFAULT '',
        threshold_ms  INTEGER,
        port          INTEGER,
        heartbeat     TEXT NOT NULL DEFAULT '',
        pid           INTEGER,
        freezes       INTEGER NOT NULL DEFAULT 0, -- freeze events stored
        frozen_ms     INTEGER NOT NULL DEFAULT 0,
        heartbeats    INTEGER NOT NULL DEFAULT 0, -- from ping or session_stop
        idle_messages INTEGER NOT NULL DEFAULT 0,
        ping_freezes  INTEGER NOT NULL DEFAULT 0, -- the plug-in's own count
        last_seq      INTEGER NOT NULL DEFAULT 0,
        options       TEXT NOT NULL DEFAULT ''    -- session_start JSON
    );
    CREATE INDEX idx_sessions_client ON sessions(client_id, last_seen_ms);

    -- One row per event: freeze, script_busy, mark, session_start,
    -- session_stop, or a type this server does not know.
    -- The context columns are the "where" of the freeze:
    --   layout, file, account, user: after the freeze if known, else before.
    --   from_layout: the layout before the freeze.
    --   script: before the freeze if known, else after.
    CREATE TABLE events (
        id           INTEGER PRIMARY KEY,
        session_id   TEXT NOT NULL,
        seq          INTEGER NOT NULL,
        client_id    INTEGER NOT NULL REFERENCES clients(id),
        type         TEXT NOT NULL,
        ts_ms        INTEGER NOT NULL,
        end_ms       INTEGER,
        duration_ms  INTEGER NOT NULL DEFAULT 0,
        wait_ms      INTEGER NOT NULL DEFAULT 0,
        round_trips  INTEGER NOT NULL DEFAULT 0,
        srtt_ms      INTEGER NOT NULL DEFAULT 0,
        tx_bytes     INTEGER NOT NULL DEFAULT 0,
        rx_bytes     INTEGER NOT NULL DEFAULT 0,
        tx_packets   INTEGER NOT NULL DEFAULT 0,
        rx_packets   INTEGER NOT NULL DEFAULT 0,
        connected    INTEGER NOT NULL DEFAULT 0,
        level        TEXT NOT NULL DEFAULT '',
        file         TEXT NOT NULL DEFAULT '',
        layout       TEXT NOT NULL DEFAULT '',
        layout_table TEXT NOT NULL DEFAULT '',
        from_layout  TEXT NOT NULL DEFAULT '',
        script       TEXT NOT NULL DEFAULT '',
        account      TEXT NOT NULL DEFAULT '',
        user         TEXT NOT NULL DEFAULT '',
        label        TEXT NOT NULL DEFAULT '',
        marks        TEXT NOT NULL DEFAULT '[]',
        before_json  TEXT NOT NULL DEFAULT '{}',
        after_json   TEXT NOT NULL DEFAULT '',
        raw          TEXT NOT NULL,
        UNIQUE (session_id, seq)
    );
    CREATE INDEX idx_events_ts      ON events(ts_ms);
    CREATE INDEX idx_events_file    ON events(file, ts_ms);
    CREATE INDEX idx_events_layout  ON events(layout, ts_ms);
    CREATE INDEX idx_events_script  ON events(script, ts_ms);
    CREATE INDEX idx_events_account ON events(account, ts_ms);
    CREATE INDEX idx_events_client  ON events(client_id, ts_ms);

    -- One row per local day x kind x file x layout x script x account x client.
    -- hist: freeze counts in the duration buckets of query::BUCKETS_MS.
    CREATE TABLE daily_totals (
        day          TEXT NOT NULL,             -- YYYY-MM-DD, server zone
        kind         TEXT NOT NULL,             -- 'freeze' or 'script_busy'
        file         TEXT NOT NULL,
        layout       TEXT NOT NULL,
        layout_table TEXT NOT NULL DEFAULT '',
        script       TEXT NOT NULL,
        account      TEXT NOT NULL,
        user         TEXT NOT NULL DEFAULT '',
        client_id    INTEGER NOT NULL,
        n            INTEGER NOT NULL,
        total_ms     INTEGER NOT NULL,
        wait_ms      INTEGER NOT NULL,          -- clamped to each duration
        max_ms       INTEGER NOT NULL,
        rt_sum       INTEGER NOT NULL,
        rt_max       INTEGER NOT NULL,
        hist         TEXT NOT NULL,
        PRIMARY KEY (day, kind, file, layout, script, account, client_id)
    );

    -- Days that got new events since their last rollup.
    CREATE TABLE rollup_dirty (
        day TEXT PRIMARY KEY
    );
    "#,
    // 2: a long freeze that still lasts. The plug-in sends `freeze_progress`
    // events; the server keeps one row for the freeze, open = 1, with
    // seq = -freeze_id (a plug-in seq is never below 1). The final `freeze`
    // event (same session, same ts) replaces it: real seq, open = 0.
    r#"
    ALTER TABLE events ADD COLUMN freeze_id INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE events ADD COLUMN open      INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE events ADD COLUMN parts     INTEGER NOT NULL DEFAULT 0;
    CREATE UNIQUE INDEX idx_events_freeze ON events(session_id, freeze_id) WHERE freeze_id > 0;
    CREATE INDEX idx_events_open ON events(open) WHERE open = 1;
    "#,
];

pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// Open (or make) the database file and run the migrations.
pub fn open(path: &Path) -> Result<Db, String> {
    if let Some(d) = path.parent() {
        if !d.as_os_str().is_empty() {
            std::fs::create_dir_all(d).map_err(|e| format!("cannot make {}: {e}", d.display()))?;
        }
    }
    let c = Connection::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    setup(c)
}

/// An in-memory database (tests).
pub fn open_memory() -> Db {
    setup(Connection::open_in_memory().unwrap()).unwrap()
}

fn setup(mut c: Connection) -> Result<Db, String> {
    let e = |e: rusqlite::Error| e.to_string();
    c.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(e)?;
    c.pragma_update(None, "journal_mode", "WAL").map_err(e)?;
    c.pragma_update(None, "synchronous", "NORMAL").map_err(e)?;
    c.pragma_update(None, "foreign_keys", "ON").map_err(e)?;
    migrate(&mut c)?;
    Ok(c)
}

pub fn migrate(c: &mut Connection) -> Result<(), String> {
    let have: i64 = c
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if have > SCHEMA_VERSION {
        return Err(format!(
            "the database has schema version {have}; this server knows up to {SCHEMA_VERSION}"
        ));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(have as usize) {
        let v = i as i64 + 1;
        let tx = c.transaction().map_err(|e| e.to_string())?;
        tx.execute_batch(sql)
            .map_err(|e| format!("migration {v}: {e}"))?;
        tx.pragma_update(None, "user_version", v)
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn get_setting(c: &Db, key: &str) -> Option<String> {
    c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
        r.get(0)
    })
    .optional()
    .ok()
    .flatten()
}

pub fn set_setting(c: &Db, key: &str, value: &str) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// The settings that the Setup page can change.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub event_days: u32,
    pub total_days: u32,
    pub flag_p95_ms: u64,
    pub flag_quiet_min: u64,
    pub flag_plugin_min: String,
    pub rec_threshold_ms: u64,
    pub rec_port: u16,
    pub rec_send_ms: u64,
    pub rec_ping_ms: u64,
}

impl Settings {
    pub fn load(c: &Db, d: &SettingDefaults) -> Settings {
        fn num<T: std::str::FromStr>(c: &Db, k: &str, def: T) -> T {
            get_setting(c, k)
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(def)
        }
        Settings {
            event_days: num(c, "event_days", d.event_days).max(1),
            total_days: num(c, "total_days", d.total_days).max(1),
            flag_p95_ms: num(c, "flag_p95_ms", d.flag_p95_ms),
            flag_quiet_min: num(c, "flag_quiet_min", d.flag_quiet_min),
            flag_plugin_min: get_setting(c, "flag_plugin_min")
                .unwrap_or_else(|| d.flag_plugin_min.clone()),
            rec_threshold_ms: num(c, "rec_threshold_ms", d.rec_threshold_ms),
            rec_port: num(c, "rec_port", d.rec_port),
            rec_send_ms: num(c, "rec_send_ms", d.rec_send_ms),
            rec_ping_ms: num(c, "rec_ping_ms", d.rec_ping_ms),
        }
    }

    pub fn save(&self, c: &Db) -> rusqlite::Result<()> {
        set_setting(c, "event_days", &self.event_days.to_string())?;
        set_setting(c, "total_days", &self.total_days.to_string())?;
        set_setting(c, "flag_p95_ms", &self.flag_p95_ms.to_string())?;
        set_setting(c, "flag_quiet_min", &self.flag_quiet_min.to_string())?;
        set_setting(c, "flag_plugin_min", &self.flag_plugin_min)?;
        set_setting(c, "rec_threshold_ms", &self.rec_threshold_ms.to_string())?;
        set_setting(c, "rec_port", &self.rec_port.to_string())?;
        set_setting(c, "rec_send_ms", &self.rec_send_ms.to_string())?;
        set_setting(c, "rec_ping_ms", &self.rec_ping_ms.to_string())?;
        Ok(())
    }
}

/// 32 random hex characters.
pub fn random_hex32() -> String {
    let mut b = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok();
    if !ok {
        // No /dev/urandom: mix the clock and the process id.
        let mut x = (tz_now_nanos() ^ ((std::process::id() as u128) << 64)) | 1;
        for v in b.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *v = x as u8;
        }
    }
    b.iter().map(|v| format!("{v:02x}")).collect()
}

fn tz_now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(1)
}

pub const TOKEN_KEY: &str = "event_token";

/// The Event token. The first call makes one and keeps it.
pub fn ensure_token(c: &Db) -> rusqlite::Result<String> {
    if let Some(t) = get_setting(c, TOKEN_KEY).filter(|t| !t.is_empty()) {
        return Ok(t);
    }
    replace_token(c)
}

/// Make a new Event token. The old token stops working.
pub fn replace_token(c: &Db) -> rusqlite::Result<String> {
    let t = random_hex32();
    set_setting(c, TOKEN_KEY, &t)?;
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_run_once_and_are_versioned() {
        let dir = std::env::temp_dir().join(format!("frs-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("x.sqlite");
        {
            let c = open(&p).unwrap();
            let v: i64 = c
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap();
            assert_eq!(v, SCHEMA_VERSION);
            let mode: String = c
                .pragma_query_value(None, "journal_mode", |r| r.get(0))
                .unwrap();
            assert_eq!(mode, "wal");
            set_setting(&c, "a", "1").unwrap();
        }
        // A second open keeps the data and runs nothing again.
        let c = open(&p).unwrap();
        assert_eq!(get_setting(&c, "a").as_deref(), Some("1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_is_made_once_and_can_be_replaced() {
        let c = open_memory();
        let t1 = ensure_token(&c).unwrap();
        assert_eq!(t1.len(), 32);
        assert!(t1.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_eq!(ensure_token(&c).unwrap(), t1);
        let t2 = replace_token(&c).unwrap();
        assert_ne!(t1, t2);
        assert_eq!(ensure_token(&c).unwrap(), t2);
    }

    #[test]
    fn settings_have_defaults_and_are_kept() {
        let c = open_memory();
        let d = SettingDefaults::default();
        let mut s = Settings::load(&c, &d);
        assert_eq!(s.flag_p95_ms, 8000);
        assert_eq!(s.flag_quiet_min, 30);
        assert_eq!(s.flag_plugin_min, "0.1.0");
        assert_eq!((s.event_days, s.total_days), (30, 730));
        s.flag_p95_ms = 6500;
        s.save(&c).unwrap();
        assert_eq!(Settings::load(&c, &d).flag_p95_ms, 6500);
    }
}
