//! "Where was the user": file, layout, script, account, window, mark.
//!
//! The context is read on FileMaker's main thread (the only thread where the
//! calc engine may be called) through a [`ContextSource`]. The plug-in glue
//! implements it with `FM_ExprEnv_EvaluateGetFunction`; tests use a fake.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub layout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub layout_table: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub script: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub window: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// Get(WindowMode): 0 browse, 1 find, 2 preview, 3 printing, 4 layout.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// Monotonic ms at capture (to know how stale the "before" context is).
    #[serde(default)]
    pub captured_ms: u64,
    /// How long the capture took, microseconds (feasibility question b).
    #[serde(default)]
    pub capture_us: u64,
}

impl Context {
    pub fn is_empty(&self) -> bool {
        self.file.is_empty() && self.layout.is_empty() && self.script.is_empty()
    }
}

/// The fields we ask FileMaker for, with their `kGet_*` ids from
/// FMXCalcEngine.h (FileMaker 18+, API version 60).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    File = 1005,
    Host = 1008,
    Layout = 1010,
    WindowMode = 1013,
    Script = 1026,
    UserName = 1031,
    Account = 1032,
    Window = 1048,
    LayoutTable = 1061,
}

pub const ALL_FIELDS: [Field; 9] = [
    Field::File,
    Field::Layout,
    Field::LayoutTable,
    Field::Script,
    Field::Account,
    Field::UserName,
    Field::Window,
    Field::Host,
    Field::WindowMode,
];

/// Something that can answer `Get(...)` on the main thread.
pub trait ContextSource {
    fn get(&mut self, field: Field) -> Option<String>;
}

/// Read all fields. `now_ms` and the clock are injected for tests.
pub fn capture(
    src: &mut dyn ContextSource,
    now_ms: u64,
    clock_us: &mut dyn FnMut() -> u64,
) -> Context {
    let t0 = clock_us();
    let mut c = Context {
        captured_ms: now_ms,
        ..Default::default()
    };
    for f in ALL_FIELDS {
        let v = src.get(f).unwrap_or_default();
        match f {
            Field::File => c.file = v,
            Field::Host => c.host = v,
            Field::Layout => c.layout = v,
            Field::WindowMode => c.mode = v,
            Field::Script => c.script = v,
            Field::UserName => c.user = v,
            Field::Account => c.account = v,
            Field::Window => c.window = v,
            Field::LayoutTable => c.layout_table = v,
        }
    }
    c.capture_us = clock_us().saturating_sub(t0);
    c
}

/// Rate limit for context capture during idle. Reading 9 Get() values costs
/// something; we do it at most every `interval_ms`, and never when FileMaker
/// says calls are unsafe.
#[derive(Debug, Clone)]
pub struct CaptureGate {
    interval_ms: u64,
    last_ms: Option<u64>,
}

impl CaptureGate {
    pub fn new(interval_ms: u64) -> Self {
        Self {
            interval_ms,
            last_ms: None,
        }
    }
    pub fn should_capture(&mut self, now_ms: u64) -> bool {
        match self.last_ms {
            Some(l) if now_ms.saturating_sub(l) < self.interval_ms => false,
            _ => {
                self.last_ms = Some(now_ms);
                true
            }
        }
    }
    /// Force the next call to capture (after a stall, a layout may have changed).
    pub fn reset(&mut self) {
        self.last_ms = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(u32);
    impl ContextSource for Fake {
        fn get(&mut self, f: Field) -> Option<String> {
            self.0 += 1;
            Some(match f {
                Field::File => "Invoices".into(),
                Field::Layout => "Invoice Detail".into(),
                Field::Script => "OnRecordLoad".into(),
                Field::Account => "jose".into(),
                Field::WindowMode => "0".into(),
                _ => return None,
            })
        }
    }

    #[test]
    fn capture_reads_every_field_once() {
        let mut f = Fake(0);
        let mut t = 100u64;
        let mut clock = || {
            t += 40;
            t
        };
        let c = capture(&mut f, 5_000, &mut clock);
        assert_eq!(f.0 as usize, ALL_FIELDS.len());
        assert_eq!(c.file, "Invoices");
        assert_eq!(c.layout, "Invoice Detail");
        assert_eq!(c.script, "OnRecordLoad");
        assert_eq!(c.account, "jose");
        assert_eq!(c.window, "");
        assert_eq!(c.captured_ms, 5_000);
        assert_eq!(c.capture_us, 40);
        assert!(!c.is_empty());
    }

    #[test]
    fn empty_fields_are_not_serialized() {
        let c = Context {
            layout: "L".into(),
            ..Default::default()
        };
        let j = serde_json::to_string(&c).unwrap();
        assert!(j.contains("\"layout\":\"L\""));
        assert!(!j.contains("script"));
    }

    #[test]
    fn gate_limits_rate() {
        let mut g = CaptureGate::new(250);
        assert!(g.should_capture(0));
        assert!(!g.should_capture(100));
        assert!(g.should_capture(250));
        assert!(!g.should_capture(300));
        g.reset();
        assert!(g.should_capture(301));
    }
}
