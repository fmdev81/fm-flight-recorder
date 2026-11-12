//! The saved `FlightRecorder_Start` options, so sending survives a relaunch.
//!
//! * A start with `endpoint=` saves its option text to
//!   `<state dir>/start-options` (mode 0600: it holds the token).
//! * At launch (`kFMXT_Init`) the plug-in starts with the saved text, or
//!   with no options when there is no file (or the file is not valid).
//! * `FlightRecorder_Stop("forget")` deletes the file.
//! * A start with the same options as the running session keeps that
//!   session (no stop, no new session). Repeated calls, for example from a
//!   Data Viewer watch, then do not make a new session each time.
//!
//! The file is one short line, so reading and writing it on FileMaker's
//! main thread costs about as much as one `stat`.

use crate::recorder::{Config, OptionReport};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// File name in the state folder.
pub const FILE_NAME: &str = "start-options";
/// A larger file is not ours: ignore it.
const MAX_BYTES: u64 = 64 * 1024;

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// One form for the same options: each `key=value` part trimmed, empty
/// parts dropped, joined with "; ". The order stays (a later key wins in
/// [`Config::apply_options`], so the order can matter).
pub fn normalize(text: &str) -> String {
    text.split([';', '\n', '\r', ','])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Write the options (normalized) with mode 0600, through a temporary file
/// and a rename, so a reader never sees half a file.
pub fn save(dir: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    if dir.as_os_str().is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no home folder"));
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let tmp = dir.join(format!(".{FILE_NAME}.{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    // The umask can only remove bits, but be explicit.
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    let r = f
        .write_all(format!("{}\n", normalize(text)).as_bytes())
        .and_then(|_| f.sync_all())
        .and_then(|_| fs::rename(&tmp, path(dir)));
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// The saved options, or None (no file, empty, too large, not readable).
pub fn load(dir: &Path) -> Option<String> {
    if dir.as_os_str().is_empty() {
        return None;
    }
    let p = path(dir);
    if fs::metadata(&p).ok()?.len() > MAX_BYTES {
        return None;
    }
    let s = normalize(&fs::read_to_string(p).ok()?);
    (!s.is_empty()).then_some(s)
}

/// Save `text` unless the file already holds it. Ok(true) if it wrote the
/// file. A start that keeps the running session calls this, so a save that
/// failed the first time is tried again (and its error shown again).
pub fn ensure_saved(dir: &Path, text: &str) -> io::Result<bool> {
    if load(dir).as_deref() == Some(normalize(text).as_str()) {
        return Ok(false);
    }
    save(dir, text).map(|_| true)
}

/// Delete the saved options. Ok(true) if a file was there.
pub fn clear(dir: &Path) -> io::Result<bool> {
    if dir.as_os_str().is_empty() {
        return Ok(false);
    }
    match fs::remove_file(path(dir)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// The options for the start at launch: the saved text if it is still
/// valid (no errors), else "" (today's behavior: record, do not send).
pub fn init_options(dir: &Path) -> String {
    match load(dir) {
        Some(s) if Config::default().apply_options(&s).errors.is_empty() => s,
        _ => String::new(),
    }
}

/// What `FlightRecorder_Start` must do.
#[derive(Debug)]
pub enum Plan {
    /// Bad options: return this text, do not touch the running recorder.
    Error(String),
    /// Same options as the running session: keep it. `options` is the
    /// normalized text; `save` is true when it has an endpoint (then the
    /// file is checked, and written again if it is missing or differs).
    Keep {
        report: OptionReport,
        options: String,
        save: bool,
    },
    /// Stop the running recorder (if any) and start with `cfg`. `options`
    /// is the normalized text; `save` is true when it has an endpoint.
    Start {
        cfg: Box<Config>,
        report: OptionReport,
        options: String,
        save: bool,
    },
}

/// Decide what a start does. `running` is the normalized option text of
/// the running session, None if no recorder runs.
pub fn plan(options: &str, running: Option<&str>) -> Plan {
    let mut cfg = Config::default();
    let report = cfg.apply_options(options);
    if !report.errors.is_empty() {
        return Plan::Error(report.error_result());
    }
    let options = normalize(options);
    let save = cfg.endpoint.is_some();
    if running == Some(options.as_str()) {
        return Plan::Keep {
            report,
            options,
            save,
        };
    }
    Plan::Start {
        cfg: Box::new(cfg),
        report,
        options,
        save,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const OPTS: &str = "endpoint=http://127.0.0.1:8787; token=abc; scripts=1; send_ms=250";

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fr-startopts-{name}-{}-{}",
            std::process::id(),
            crate::identity::random_id()
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn normalize_trims_and_joins() {
        assert_eq!(
            normalize(" endpoint=http://h:8787 ;token=t;;\n scripts=1 "),
            "endpoint=http://h:8787; token=t; scripts=1"
        );
        assert_eq!(normalize(""), "");
        assert_eq!(normalize(" ; ;"), "");
    }

    #[test]
    fn save_then_load() {
        let d = tmp("save");
        assert_eq!(load(&d), None, "no file yet");
        save(&d, "endpoint=http://127.0.0.1:8787 ;token=abc").unwrap();
        assert_eq!(
            load(&d).as_deref(),
            Some("endpoint=http://127.0.0.1:8787; token=abc")
        );
        // A second save replaces the first, and leaves no temporary file.
        save(&d, OPTS).unwrap();
        assert_eq!(load(&d).as_deref(), Some(OPTS));
        let names: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec![FILE_NAME.to_string()]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn file_mode_is_0600_and_folder_0700() {
        let d = tmp("mode");
        save(&d, OPTS).unwrap();
        let m = fs::metadata(path(&d)).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "file mode {m:o}");
        let m = fs::metadata(&d).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o700, "folder mode {m:o}");
        // An old file with a wider mode gets 0600 at the next save.
        fs::set_permissions(path(&d), fs::Permissions::from_mode(0o644)).unwrap();
        save(&d, OPTS).unwrap();
        let m = fs::metadata(path(&d)).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "file mode after resave {m:o}");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn clear_deletes_the_file() {
        let d = tmp("clear");
        assert!(!clear(&d).unwrap(), "nothing to clear");
        save(&d, OPTS).unwrap();
        assert!(clear(&d).unwrap());
        assert_eq!(load(&d), None);
        assert_eq!(init_options(&d), "");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn no_home_folder_means_no_file() {
        let d = PathBuf::new();
        assert!(save(&d, OPTS).is_err());
        assert_eq!(load(&d), None);
        assert!(!clear(&d).unwrap());
        assert_eq!(init_options(&d), "");
    }

    #[test]
    fn init_without_a_file_records_but_does_not_send() {
        let d = tmp("init-none");
        let o = init_options(&d);
        assert_eq!(o, "");
        let Plan::Start { cfg, save, .. } = plan(&o, None) else {
            panic!("start expected")
        };
        assert!(cfg.endpoint.is_none());
        assert!(!save);
    }

    #[test]
    fn init_with_a_file_sends_again() {
        let d = tmp("init-file");
        save(&d, OPTS).unwrap();
        let o = init_options(&d);
        assert_eq!(o, OPTS);
        let Plan::Start { cfg, options, .. } = plan(&o, None) else {
            panic!("start expected")
        };
        assert_eq!(cfg.endpoint.unwrap().url(), "http://127.0.0.1:8787");
        assert_eq!(cfg.token, "abc");
        assert!(cfg.context_during_scripts);
        assert_eq!(cfg.send_interval_ms, 250);
        assert_eq!(options, OPTS);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn init_ignores_a_bad_or_empty_file() {
        let d = tmp("init-bad");
        fs::create_dir_all(&d).unwrap();
        fs::write(path(&d), "endpoint=https://h:8787; token=t\n").unwrap();
        assert_eq!(init_options(&d), "", "https is an error: do not use it");
        fs::write(path(&d), "endpoint=http://h:8787\n").unwrap();
        assert_eq!(init_options(&d), "", "no token is an error");
        fs::write(path(&d), "\n").unwrap();
        assert_eq!(init_options(&d), "");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn same_options_keep_the_session() {
        // Same text, other spaces and separators: keep.
        let running = normalize(OPTS);
        let again = "endpoint=http://127.0.0.1:8787;token=abc;scripts=1;send_ms=250;";
        let Plan::Keep { options, save, .. } = plan(again, Some(&running)) else {
            panic!("keep expected")
        };
        assert_eq!(options, running);
        assert!(save, "an endpoint: the file is checked");
        // No options while no-options runs: keep too, nothing to save.
        assert!(matches!(plan("", Some("")), Plan::Keep { save: false, .. }));
    }

    #[test]
    fn ensure_saved_writes_a_missing_or_different_file_only() {
        let d = tmp("ensure");
        // The first save failed (no file): a keep writes it.
        assert!(ensure_saved(&d, OPTS).unwrap());
        assert_eq!(load(&d).as_deref(), Some(normalize(OPTS).as_str()));
        // Same text (other separators): no write.
        let t0 = fs::metadata(path(&d)).unwrap().modified().unwrap();
        assert!(!ensure_saved(&d, &OPTS.replace("; ", ";")).unwrap());
        assert_eq!(fs::metadata(path(&d)).unwrap().modified().unwrap(), t0);
        // The file differs (another start wrote it, or it was edited): write.
        save(&d, "endpoint=http://other:8787; token=x").unwrap();
        assert!(ensure_saved(&d, OPTS).unwrap());
        assert_eq!(load(&d).as_deref(), Some(normalize(OPTS).as_str()));
        // A folder that cannot be written: the error comes back every time.
        assert!(ensure_saved(&PathBuf::new(), OPTS).is_err());
        assert!(ensure_saved(&PathBuf::new(), OPTS).is_err());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn other_options_restart() {
        let running = normalize(OPTS);
        let Plan::Start { options, save, .. } = plan(
            "endpoint=http://127.0.0.1:8787; token=abc; scripts=0",
            Some(&running),
        ) else {
            panic!("start expected")
        };
        assert_eq!(
            options,
            "endpoint=http://127.0.0.1:8787; token=abc; scripts=0"
        );
        assert!(save);
        // No recorder runs: always start.
        assert!(matches!(plan(OPTS, None), Plan::Start { .. }));
        // No endpoint: start, but do not save.
        let Plan::Start { save, .. } = plan("threshold=300", Some(&running)) else {
            panic!("start expected")
        };
        assert!(!save);
    }

    #[test]
    fn bad_options_do_not_touch_the_session() {
        let running = normalize(OPTS);
        match plan("endpoint=http://h:8787", Some(&running)) {
            Plan::Error(e) => assert_eq!(e, "error: token is required with endpoint"),
            p => panic!("error expected, got {p:?}"),
        }
    }
}
