//! FileMaker plug-in glue: `FMExternCallProc` and the external functions.
//!
//! Rules this code keeps (see https://github.com/solutionscay/fm-flight-recorder/wiki/SDK-notes):
//! * FileMaker calls the entry point and the external functions on its main
//!   thread. Only that thread calls FMWrapper. The recorder threads never do.
//! * Idle handling is cheap: an atomic heartbeat, and at most every 250 ms
//!   (or right after a freeze) a context read, and only when FileMaker says
//!   calls are safe (`unsafeCalls == 0`, which SDK 26 documents as "not on
//!   the main thread"; level user idle / not idle / script paused).
//! * No panic crosses the C boundary: every entry is wrapped in catch_unwind.

mod ffi;
mod selftest;

use crate::context::{self, Context, ContextSource, Field};
use crate::fmxabi::*;
use crate::recorder::{mono_ns, Level, Recorder};
use crate::runloop::{self, RunLoopObserver};
use crate::startopts::{self, Plan};
use ffi::*;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct State {
    rec: Arc<Recorder>,
    observer: Option<RunLoopObserver>,
    /// The normalized option text of this session (see startopts::plan).
    options: String,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static APP_VERSION: Mutex<String> = Mutex::new(String::new());
/// FMX_CurrentEnvCall from the call struct, as usize (0 = absent).
static CURRENT_ENV: AtomicUsize = AtomicUsize::new(0);

// ---------- small RAII wrappers over FMWrapper objects ----------

struct FmText(*mut Text);
impl FmText {
    fn new(s: &str) -> Self {
        let mut x = Fmxcpt::new();
        let t = unsafe { FM_Text_Constructor1(&mut x) };
        let u: Vec<u16> = s.encode_utf16().collect();
        unsafe { FM_Text_AssignUnicodeWithLength(t, u.as_ptr(), u.len() as u32, &mut x) };
        FmText(t)
    }
}
impl Drop for FmText {
    fn drop(&mut self) {
        let mut x = Fmxcpt::new();
        unsafe { FM_Text_Delete(self.0, &mut x) }
    }
}

fn text_to_string(t: *const Text) -> String {
    if t.is_null() {
        return String::new();
    }
    let mut x = Fmxcpt::new();
    let n = unsafe { FM_Text_GetSize(t, &mut x) } as usize;
    if n == 0 {
        return String::new();
    }
    let mut buf = vec![0u16; n];
    unsafe { FM_Text_GetUnicode(t, buf.as_mut_ptr(), 0, n as u32, &mut x) };
    String::from_utf16_lossy(&buf)
}

fn set_result_text(result: *mut Data, s: &str) {
    if result.is_null() {
        return;
    }
    let t = FmText::new(s);
    let mut x = Fmxcpt::new();
    unsafe {
        let loc = FM_Locale_Constructor1(K_LOCALE_SYSTEM, &mut x);
        FM_Data_SetAsText(result, t.0, loc, K_DT_TEXT, &mut x);
        FM_Locale_Delete(loc, &mut x);
    }
}

fn param_text(parms: *const DataVect, i: u32) -> Option<String> {
    let mut x = Fmxcpt::new();
    unsafe {
        if parms.is_null() || FM_DataVect_Size(parms, &mut x) <= i {
            return None;
        }
        Some(text_to_string(FM_DataVect_AtAsText(parms, i, &mut x)))
    }
}

fn quad() -> *mut QuadChar {
    let mut x = Fmxcpt::new();
    let [a, b, c, d] = PLUGIN_ID;
    unsafe { FM_QuadChar_Constructor2(a as i8, b as i8, c as i8, d as i8, &mut x) }
}

/// Answers Get() through an ExprEnv.
struct EnvSource(*const ExprEnv);
impl ContextSource for EnvSource {
    fn get(&mut self, f: Field) -> Option<String> {
        let mut x = Fmxcpt::new();
        unsafe {
            let d = FM_Data_Constructor1(&mut x);
            let err = FM_ExprEnv_EvaluateGetFunction(self.0, f as i16, d, &mut x);
            let s = if err == 0 && x.m_code == 0 {
                Some(text_to_string(FM_Data_GetAsText(d, &mut x)))
            } else {
                None
            };
            FM_Data_Delete(d, &mut x);
            s
        }
    }
}

fn now_us() -> u64 {
    mono_ns() / 1000
}

fn capture_with(env: *const ExprEnv) -> Context {
    let mut src = EnvSource(env);
    context::capture(&mut src, mono_ns() / 1_000_000, &mut || now_us())
}

/// Capture during idle: build an ExprEnv and point it at the current
/// window's file with FMX_SetToCurrentEnv (cCurrentEnv, API 62).
fn capture_current() -> Option<Context> {
    let f = CURRENT_ENV.load(Ordering::Relaxed);
    if f == 0 {
        return None;
    }
    let set_env: unsafe extern "C" fn(*mut core::ffi::c_void) -> i16 =
        unsafe { std::mem::transmute(f) };
    let mut x = Fmxcpt::new();
    unsafe {
        let env = FM_ExprEnv_Constructor1(&mut x);
        if env.is_null() {
            return None;
        }
        let ctx = if set_env(env) == 0 {
            Some(capture_with(env))
        } else {
            None
        };
        FM_ExprEnv_Delete(env, &mut x);
        ctx
    }
}

// ---------- recorder lifecycle ----------

/// Returns the log path (plus "ignored: …" for unknown options), or
/// "error: …" when the options are bad. On error the running recorder is
/// not touched. The same options as the running session keep it. A start
/// with an endpoint saves its options for the next launch.
fn start(options: &str) -> String {
    let mut st = STATE.lock().unwrap();
    let running = st
        .as_ref()
        .filter(|s| s.rec.is_running())
        .map(|s| s.options.as_str());
    let (mut cfg, report, options, save) = match startopts::plan(options, running) {
        Plan::Error(e) => return e,
        Plan::Keep(report) => {
            let path = st.as_ref().map(|s| s.rec.log_path().display().to_string());
            return report.start_result(&path.unwrap_or_default());
        }
        Plan::Start {
            cfg,
            report,
            options,
            save,
        } => (*cfg, report, options, save),
    };
    if let Some(old) = st.take() {
        drop(old.observer);
        old.rec.stop();
    }
    cfg.app = APP_VERSION.lock().unwrap().clone();
    cfg.heartbeat = "idle+runloop".into();
    let rec = Arc::new(Recorder::start(
        cfg.clone(),
        crate::default_probe(cfg.net_port),
    ));
    // We are on the main thread (Init or an external function call).
    // The watchdog checks that this thread really sleeps when the run loop
    // says "waiting" (a stall can follow BeforeWaiting in the same pass).
    rec.watch_current_thread();
    let r2 = rec.clone();
    let observer = unsafe {
        RunLoopObserver::install(runloop::CFRunLoopGetMain(), move |a| runloop::feed(&r2, a))
    };
    let path = rec.log_path().display().to_string();
    *st = Some(State {
        rec,
        observer,
        options: options.clone(),
    });
    let mut out = report.start_result(&path);
    if save {
        if let Err(e) = startopts::save(&cfg.state_dir, &options) {
            out.push_str(&format!("\nnot saved for the next launch: {e}"));
        }
    }
    out
}

/// `FlightRecorder_Stop( { "forget" } )`. With "forget", also delete the
/// saved options, so the next launch records but does not send.
fn stop(arg: &str) -> String {
    let forget = match arg.trim().to_ascii_lowercase().as_str() {
        "" => false,
        "forget" => true,
        other => return format!("error: unknown argument '{other}', use \"forget\""),
    };
    let mut out = match STATE.lock().unwrap().take() {
        None => "not running".to_string(),
        Some(old) => {
            drop(old.observer);
            let s = old.rec.stop();
            format!(
                "stopped: {} freezes, {} heartbeats, log {}",
                s.freezes, s.heartbeats, s.log
            )
        }
    };
    if forget {
        let dir = crate::identity::default_state_dir();
        out.push_str(&match startopts::clear(&dir) {
            Ok(true) => "\nsaved options deleted".to_string(),
            Ok(false) => "\nno saved options".to_string(),
            Err(e) => format!("\nsaved options not deleted: {e}"),
        });
    }
    out
}

fn recorder() -> Option<Arc<Recorder>> {
    STATE.lock().ok()?.as_ref().map(|s| s.rec.clone())
}

// ---------- external functions ----------

unsafe extern "C" fn ext_function(
    id: i16,
    env: *const ExprEnv,
    parms: *const DataVect,
    result: *mut Data,
) -> i16 {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let out = match id {
            1 => crate::VERSION.to_string(),
            2 => start(&param_text(parms, 0).unwrap_or_default()),
            3 => stop(&param_text(parms, 0).unwrap_or_default()),
            4 => {
                let label = param_text(parms, 0).unwrap_or_default();
                match recorder() {
                    Some(r) => {
                        r.mark(&label, capture_with(env));
                        "1".into()
                    }
                    None => "0".into(),
                }
            }
            5 => match recorder() {
                Some(r) => serde_json::to_string(&r.status()).unwrap_or_default(),
                None => "{\"running\":false}".into(),
            },
            6 => {
                let ms = param_text(parms, 0)
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .unwrap_or(0.0);
                let ms = ms.clamp(0.0, 10_000.0) as u64;
                std::thread::sleep(std::time::Duration::from_millis(ms));
                ms.to_string()
            }
            _ => return 960, // "Parameter is invalid"
        };
        set_result_text(result, &out);
        0
    }));
    r.unwrap_or(960)
}

fn register_all() -> bool {
    let q = quad();
    let mut ok = true;
    for &(id, name, proto, desc, min, max) in FUNCTIONS {
        let (n, p, d) = (FmText::new(name), FmText::new(proto), FmText::new(desc));
        let mut x = Fmxcpt::new();
        let err = unsafe {
            FM_ExprEnv_RegisterExternalFunctionEx(
                q,
                id,
                n.0,
                p.0,
                d.0,
                min,
                max,
                K_DISPLAY_IN_ALL_DIALOGS | K_MAC_COMPATIBLE,
                ext_function,
                &mut x,
            )
        };
        ok &= err == 0 && x.m_code == 0;
    }
    let mut x = Fmxcpt::new();
    unsafe { FM_QuadChar_Delete(q, &mut x) };
    ok
}

fn unregister_all() {
    let q = quad();
    for &(id, ..) in FUNCTIONS {
        let mut x = Fmxcpt::new();
        unsafe { FM_ExprEnv_UnRegisterExternalFunction(q, id, &mut x) };
    }
    let mut x = Fmxcpt::new();
    unsafe { FM_QuadChar_Delete(q, &mut x) };
}

// ---------- entry point ----------

fn on_idle(pb: &ExternCallStruct) {
    let Some(rec) = recorder() else { return };
    let level = Level::from_u8(pb.parm1);
    rec.beat(level);
    if pb.unsafe_calls == 0 && rec.wants_context(level) {
        if let Some(ctx) = capture_current() {
            rec.offer_context(ctx);
        }
    }
    if pb.unsafe_calls == 0 && level.safe_for_calls(false) && selftest::enabled() {
        selftest::tick(&rec);
    }
}

/// The plug-in entry point. FileMaker finds it by name in the bundle.
///
/// # Safety
/// Called by FileMaker with a valid FMX_ExternCallStruct.
#[no_mangle]
pub unsafe extern "C" fn FMExternCallProc(pb: *mut ExternCallStruct) {
    if pb.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let p = &mut *pb;
        match p.which_call {
            K_INIT => {
                if p.extn_version < K_MIN_SUPPORTED_EXTN_VERSION {
                    p.result = K_DO_NOT_ENABLE as u64;
                    return;
                }
                CURRENT_ENV.store(
                    p.c_current_env.map(|f| f as usize).unwrap_or(0),
                    Ordering::Relaxed,
                );
                *APP_VERSION.lock().unwrap() = read_utf16z(p.parm2 as *const u16);
                if !register_all() {
                    p.result = K_DO_NOT_ENABLE as u64;
                    return;
                }
                // Record from launch: with the options of the last start
                // that had an endpoint, else with the default options.
                if std::env::var_os("FLIGHT_RECORDER_NO_AUTOSTART").is_none() {
                    start(&startopts::init_options(
                        &crate::identity::default_state_dir(),
                    ));
                    if let Some(r) = recorder() {
                        r.mark(
                            &format!(
                                "init: extnVersion {} app type {} app '{}' cCurrentEnv {} selftest {} saved options {}",
                                { p.extn_version },
                                p.parm1,
                                APP_VERSION.lock().unwrap(),
                                CURRENT_ENV.load(Ordering::Relaxed) != 0,
                                selftest::enabled(),
                                r.config().endpoint.is_some()
                            ),
                            Context::default(),
                        );
                    }
                }
                p.result = K_CURRENT_EXTN_VERSION;
            }
            K_IDLE => on_idle(p),
            K_SHUTDOWN => {
                stop("");
                unregister_all();
            }
            K_GET_STRING => {
                let s = get_string(p.parm1);
                write_utf16(p.result as *mut u16, p.parm3 as usize, &s);
            }
            _ => {}
        }
    }));
}
