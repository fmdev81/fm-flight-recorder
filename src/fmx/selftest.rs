//! Opt-in self-test, for checking the plug-in inside FileMaker without
//! clicking. Enabled only when FileMaker is started with the environment
//! variable FLIGHT_RECORDER_SELFTEST=1.
//!
//! Once a file is open, at safe idle messages, it evaluates calculations
//! through FileMaker's own calc engine (FM_ExprEnv_Evaluate on the current
//! window's ExprEnv) and writes each result to the log as a `mark`:
//!   0  FlightRecorder_Version, Get(ApplicationVersion), Get(FileName)/Get(LayoutName), FlightRecorder_Status
//!   1  FlightRecorder_SimulateFreeze ( 1500 )        -> a freeze the recorder should log
//!   2  a While() loop that keeps the calc engine busy -> a freeze caused by FileMaker itself
//!   3  FlightRecorder_Mark ( "called from calc" ), final status, "selftest: done"

use super::ffi::*;
use super::{capture_current, text_to_string, FmText, CURRENT_ENV};
use crate::recorder::{mono_ns, Recorder};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering::Relaxed};

static STEP: AtomicU8 = AtomicU8::new(0);
static NEXT_MS: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    std::env::var_os("FLIGHT_RECORDER_SELFTEST").is_some_and(|v| v == "1")
}

/// Evaluate `expr` in the current window's file. Returns (error, result text).
fn evaluate(expr: &str) -> (i16, String) {
    let f = CURRENT_ENV.load(Relaxed);
    if f == 0 {
        return (-1, "no cCurrentEnv".into());
    }
    let set_env: unsafe extern "C" fn(*mut core::ffi::c_void) -> i16 =
        unsafe { std::mem::transmute(f) };
    let mut x = Fmxcpt::new();
    unsafe {
        let env = FM_ExprEnv_Constructor1(&mut x);
        let e = set_env(env);
        if e != 0 {
            FM_ExprEnv_Delete(env, &mut x);
            return (e, "cCurrentEnv failed".into());
        }
        let t = FmText::new(expr);
        let d = FM_Data_Constructor1(&mut x);
        let err = FM_ExprEnv_Evaluate(env, t.0, d, &mut x);
        let s = text_to_string(FM_Data_GetAsText(d, &mut x));
        FM_Data_Delete(d, &mut x);
        FM_ExprEnv_Delete(env, &mut x);
        (err, s)
    }
}

fn mark(rec: &Recorder, label: String) {
    rec.mark(&label, capture_current().unwrap_or_default());
}

fn eval_mark(rec: &Recorder, expr: &str) {
    let t0 = mono_ns();
    let (err, s) = evaluate(expr);
    let ms = (mono_ns() - t0) / 1_000_000;
    mark(
        rec,
        format!("selftest: {expr} => err {err}, {ms} ms, result {s}"),
    );
}

/// Called from on_idle at safe levels on the main thread.
pub fn tick(rec: &Recorder) {
    let now = mono_ns() / 1_000_000;
    if now < NEXT_MS.load(Relaxed) {
        return;
    }
    match STEP.load(Relaxed) {
        0 => {
            let (err, file) = evaluate("Get ( FileName )");
            if err != 0 || file.is_empty() {
                NEXT_MS.store(now + 500, Relaxed);
                return;
            }
            for e in [
                "FlightRecorder_Version",
                "Get ( ApplicationVersion )",
                "Get ( FileName ) & \" / \" & Get ( LayoutName ) & \" / \" & Get ( AccountName )",
                "FlightRecorder_Status",
            ] {
                eval_mark(rec, e);
            }
        }
        1 => eval_mark(rec, "FlightRecorder_SimulateFreeze ( 1500 )"),
        2 => eval_mark(
            rec,
            "While ( [ i = 0 ; s = \"\" ] ; i < 49000 ; [ i = i + 1 ; s = s & \"x\" & i ] ; Length ( s ) )",
        ),
        3 => {
            eval_mark(rec, "FlightRecorder_Mark ( \"called from calc\" )");
            eval_mark(rec, "FlightRecorder_Status");
            mark(rec, "selftest: done".into());
        }
        _ => return,
    }
    STEP.fetch_add(1, Relaxed);
    NEXT_MS.store(mono_ns() / 1_000_000 + 2500, Relaxed);
}
