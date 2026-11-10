//! Server configuration: defaults, an optional TOML file, and command-line
//! flags. A flag wins over the file; the file wins over the default.
//!
//! The values that the Setup page can change (flag thresholds, retention,
//! the recording options of the startup step) are kept in the database
//! `settings` table. The config values are their first values only.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_LISTEN: &str = "0.0.0.0:8787";
pub const DB_FILE: &str = "flight-recorder.sqlite";
pub const CONFIG_FILE: &str = "config.toml";

/// Everything the server reads at start.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address and port to listen on.
    pub listen: String,
    /// Folder for the database and the default config file.
    pub data_dir: PathBuf,
    /// The SQLite database file.
    pub db: PathBuf,
    /// IANA time zone name for days and hours. Empty = this machine's zone.
    pub tz: String,
    /// Log file. None = stdout.
    pub log_file: Option<PathBuf>,
    /// The address that the plug-in uses (`endpoint=`). Empty = found from
    /// the LAN address of this machine.
    pub public_url: String,
    /// A client is connected when a ping or a batch came in this time.
    pub connected_secs: u64,
    /// First values of the settings (the Setup page can change them).
    pub defaults: SettingDefaults,
}

/// First values of the settings that the Setup page can change.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingDefaults {
    pub event_days: u32,
    pub total_days: u32,
    pub flag_p95_ms: u64,
    pub flag_quiet_min: u64,
    pub flag_plugin_min: String,
    pub rec_threshold_ms: u64,
    pub rec_port: u16,
    /// `send_ms=` and `ping_ms=` in the startup script step. The defaults
    /// are the live values: a freeze is on the dashboard about 1 s after it
    /// ends. The plug-in's own defaults (5000, 60000) are for no option.
    pub rec_send_ms: u64,
    pub rec_ping_ms: u64,
}

impl Default for SettingDefaults {
    fn default() -> Self {
        SettingDefaults {
            event_days: 30,
            total_days: 730,
            flag_p95_ms: 8000,
            flag_quiet_min: 30,
            flag_plugin_min: "0.1.0".into(),
            rec_threshold_ms: 250,
            rec_port: 5003,
            rec_send_ms: 250,
            rec_ping_ms: 15_000,
        }
    }
}

/// The TOML file. All keys are optional.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub listen: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub db: Option<PathBuf>,
    pub tz: Option<String>,
    pub log_file: Option<PathBuf>,
    pub public_url: Option<String>,
    pub connected_secs: Option<u64>,
    pub event_days: Option<u32>,
    pub total_days: Option<u32>,
    pub flag_p95_ms: Option<u64>,
    pub flag_quiet_min: Option<u64>,
    pub flag_plugin_min: Option<String>,
    pub threshold_ms: Option<u64>,
    pub port: Option<u16>,
    pub send_ms: Option<u64>,
    pub ping_ms: Option<u64>,
}

/// Command-line values (all optional; see `main.rs`).
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub config: Option<PathBuf>,
    pub listen: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub db: Option<PathBuf>,
    pub tz: Option<String>,
    pub log_file: Option<PathBuf>,
    pub public_url: Option<String>,
}

/// `~/Library/Application Support/FlightRecorder Server` on macOS,
/// `$XDG_DATA_HOME/flight-recorder-server` or `~/.local/share/...` elsewhere.
pub fn default_data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FR_SERVER_DATA_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/FlightRecorder Server")
    } else if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(x).join("flight-recorder-server")
    } else {
        home.join(".local/share/flight-recorder-server")
    }
}

impl Config {
    /// Defaults, then the config file (if any), then the flags.
    pub fn load(o: &Overrides) -> Result<Config, String> {
        let data_dir_hint = o.data_dir.clone().unwrap_or_else(default_data_dir);
        let (file, explicit) = match &o.config {
            Some(p) => (Some(p.clone()), true),
            None => (Some(data_dir_hint.join(CONFIG_FILE)), false),
        };
        let fc = match file {
            Some(p) if p.exists() => read_file(&p)?,
            Some(p) if explicit => return Err(format!("config file not found: {}", p.display())),
            _ => FileConfig::default(),
        };
        Ok(Self::merge(fc, o))
    }

    pub fn merge(fc: FileConfig, o: &Overrides) -> Config {
        let data_dir = o
            .data_dir
            .clone()
            .or(fc.data_dir)
            .unwrap_or_else(default_data_dir);
        let db =
            o.db.clone()
                .or(fc.db)
                .unwrap_or_else(|| data_dir.join(DB_FILE));
        let d = SettingDefaults::default();
        Config {
            listen: o
                .listen
                .clone()
                .or(fc.listen)
                .unwrap_or_else(|| DEFAULT_LISTEN.into()),
            db,
            data_dir,
            tz: o.tz.clone().or(fc.tz).unwrap_or_default(),
            log_file: o.log_file.clone().or(fc.log_file),
            public_url: o.public_url.clone().or(fc.public_url).unwrap_or_default(),
            connected_secs: fc.connected_secs.unwrap_or(120).max(1),
            defaults: SettingDefaults {
                event_days: fc.event_days.unwrap_or(d.event_days).max(1),
                total_days: fc.total_days.unwrap_or(d.total_days).max(1),
                flag_p95_ms: fc.flag_p95_ms.unwrap_or(d.flag_p95_ms),
                flag_quiet_min: fc.flag_quiet_min.unwrap_or(d.flag_quiet_min),
                flag_plugin_min: fc.flag_plugin_min.unwrap_or(d.flag_plugin_min),
                rec_threshold_ms: fc.threshold_ms.unwrap_or(d.rec_threshold_ms),
                rec_port: fc.port.unwrap_or(d.rec_port),
                rec_send_ms: fc.send_ms.unwrap_or(d.rec_send_ms).clamp(250, 60_000),
                rec_ping_ms: fc.ping_ms.unwrap_or(d.rec_ping_ms).clamp(5_000, 600_000),
            },
        }
    }

    /// A config for tests: a database in `dir`, a fixed zone.
    pub fn for_test(dir: &Path, tz: &str) -> Config {
        let o = Overrides {
            data_dir: Some(dir.to_path_buf()),
            tz: Some(tz.into()),
            listen: Some("127.0.0.1:0".into()),
            ..Default::default()
        };
        Self::merge(FileConfig::default(), &o)
    }
}

fn read_file(p: &Path) -> Result<FileConfig, String> {
    let text =
        std::fs::read_to_string(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_win_over_file_and_file_over_defaults() {
        let fc: FileConfig = toml::from_str(
            "listen = \"127.0.0.1:9000\"\ntz = \"America/Chicago\"\nevent_days = 14\nflag_p95_ms = 5000\n",
        )
        .unwrap();
        let o = Overrides {
            listen: Some("0.0.0.0:8800".into()),
            data_dir: Some("/tmp/x".into()),
            ..Default::default()
        };
        let c = Config::merge(fc, &o);
        assert_eq!(c.listen, "0.0.0.0:8800");
        assert_eq!(c.tz, "America/Chicago");
        assert_eq!(c.db, PathBuf::from("/tmp/x").join(DB_FILE));
        assert_eq!(c.defaults.event_days, 14);
        assert_eq!(c.defaults.total_days, 730);
        assert_eq!(c.defaults.flag_p95_ms, 5000);
    }

    #[test]
    fn defaults() {
        let c = Config::merge(FileConfig::default(), &Overrides::default());
        assert_eq!(c.listen, DEFAULT_LISTEN);
        assert_eq!(c.connected_secs, 120);
        assert_eq!(c.defaults, SettingDefaults::default());
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert!(toml::from_str::<FileConfig>("lisen = \"x\"").is_err());
    }
}
