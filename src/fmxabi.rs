//! FileMaker plug-in ABI facts that need no FileMaker library: the call
//! struct layout, message constants, the options string, UTF-16 helpers.
//!
//! Written from the FMWrapper headers of the Claris plug-in SDK
//! (FMXExtern.h, FMXCalcEngine.h). Checked against SDK 26.0.3.307
//! (fm_plugin_sdk_26.0.3.307.zip): same struct layout, kCurrentExtnVersion 77.
//! We do not copy the headers into this repo; see https://github.com/solutionscay/fm-flight-recorder/wiki/SDK-notes.

/// FMX_ExternCallStruct (FMXExtern.h). macOS: `__attribute__((packed))`.
/// On LP64 `fmx::unusedid` is int32. Size must be 74 bytes.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ExternCallStruct {
    pub extn_version: i16,
    pub unused_id: i32,
    pub entry_point: Option<unsafe extern "C" fn(*mut ExternCallStruct)>,
    pub cfm_calls: u8,
    pub which_call: u8,
    /// SDK 26: "True if not on the main thread".
    pub unsafe_calls: u8,
    pub parm1: u8,
    pub parm2: u64,
    pub parm3: u64,
    pub instance_id: u64,
    pub result: u64,
    pub unused: u64,
    /// FMX_StartScriptCall
    pub c_start_script: Option<unsafe extern "C" fn(*const u8, *const u8, u8, *const u8) -> i16>,
    /// FMX_CurrentEnvCall: errcode (*)(ExprEnv* env)
    pub c_current_env: Option<unsafe extern "C" fn(*mut core::ffi::c_void) -> i16>,
}

const _: () = assert!(core::mem::size_of::<ExternCallStruct>() == 74);

// whichCall (FMX_ExternCallSwitch)
pub const K_INIT: u8 = 0;
pub const K_IDLE: u8 = 1;
pub const K_SHUTDOWN: u8 = 4;
pub const K_DO_APP_PREFERENCES: u8 = 5;
pub const K_GET_STRING: u8 = 7;
pub const K_SESSION_SHUTDOWN: u8 = 8;
pub const K_FILE_SHUTDOWN: u8 = 9;

// GetString selectors (FMX_Strings)
pub const K_NAME_STR: u8 = 128;
pub const K_APP_CONFIG_STR: u8 = 129;
pub const K_OPTIONS_STR: u8 = 131;
pub const K_HELP_URL_STR: u8 = 132;

// Extension versions
pub const K_CURRENT_EXTN_VERSION: u64 = 77; // k260ExtnVersion (SDK 26.0.3)
pub const K_DO_NOT_ENABLE: i64 = -2;
/// cCurrentEnv (API 62, FileMaker 19) and kGet_* ids (API 60) are needed.
pub const K_MIN_SUPPORTED_EXTN_VERSION: i16 = 62;

// Function flags (FMXCalcEngine.h)
pub const K_DISPLAY_IN_ALL_DIALOGS: u32 = 0x0000_FF00;
pub const K_MAC_COMPATIBLE: u32 = 0x0000_0002;

pub const K_DT_TEXT: i32 = 1; // Data::kDTText
pub const K_LOCALE_SYSTEM: i32 = 1; // Locale::kType_System

/// 4-character plug-in id. Rules: unique, low-ASCII alphanumerics, must not
/// start with "F", "FM" or "Web" (reserved). Not registered (prototype).
pub const PLUGIN_ID: [u8; 4] = *b"SCFR";
pub const PLUGIN_NAME: &str = "FlightRecorder";

/// The options string (kFMXT_OptionsStr), 11 characters:
/// 1-4 plug-in id, 5 always '1', 6 'Y' = has a preferences button,
/// 7 always 'n', 8 'Y' = send Init/Shutdown, 9 'Y' = send Idle,
/// 10 'Y' = send Session/File shutdown, 11 always 'n'.
pub fn options_string(prefs: bool, init: bool, idle: bool, file_shutdown: bool) -> String {
    let yn = |b: bool| if b { 'Y' } else { 'n' };
    let mut s: String = PLUGIN_ID.iter().map(|&b| b as char).collect();
    s.push('1');
    s.push(yn(prefs));
    s.push('n');
    s.push(yn(init));
    s.push(yn(idle));
    s.push(yn(file_shutdown));
    s.push('n');
    s
}

/// Answer for kFMXT_GetString.
pub fn get_string(which: u8) -> String {
    match which {
        K_NAME_STR => PLUGIN_NAME.to_string(),
        K_APP_CONFIG_STR => format!(
            "FlightRecorder {} records client-side freezes (layout, script, round trips) to ~/Library/Logs/FlightRecorder.",
            crate::VERSION
        ),
        K_OPTIONS_STR => options_string(false, true, true, false),
        K_HELP_URL_STR => String::new(),
        _ => String::new(),
    }
}

/// Write `s` as NUL-terminated UTF-16 into a buffer of `cap` u16 units,
/// truncating if needed. Returns the units written, without the NUL.
///
/// # Safety
/// `buf` must be valid for `cap` u16 writes.
pub unsafe fn write_utf16(buf: *mut u16, cap: usize, s: &str) -> usize {
    if buf.is_null() || cap == 0 {
        return 0;
    }
    let mut n = 0;
    for u in s.encode_utf16() {
        if n + 1 >= cap {
            break;
        }
        *buf.add(n) = u;
        n += 1;
    }
    *buf.add(n) = 0;
    n
}

/// Read a NUL-terminated UTF-16 string (e.g. the app version passed in Init).
///
/// # Safety
/// `p` must point to a NUL-terminated u16 string or be null.
pub unsafe fn read_utf16z(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 && n < 4096 {
        n += 1;
    }
    String::from_utf16_lossy(core::slice::from_raw_parts(p, n))
}

/// Function table: (id, name, prototype, description, min, max).
pub const FUNCTIONS: &[(i16, &str, &str, &str, i16, i16)] = &[
    (1, "FlightRecorder_Version", "FlightRecorder_Version", "Plug-in version.", 0, 0),
    (
        2,
        "FlightRecorder_Start",
        "FlightRecorder_Start( { options } )",
        "Start recording. Options: \"threshold=250;port=5003;scripts=0;context_ms=250;endpoint=http://host:8787;token=…;client=name\". Returns the log path, or error: … for a bad endpoint or a missing token. With endpoint=, the options are saved and used again at the next launch. The same options as the running session keep that session.",
        0,
        1,
    ),
    (
        3,
        "FlightRecorder_Stop",
        "FlightRecorder_Stop( { \"forget\" } )",
        "Stop recording. Returns a summary. With \"forget\", also delete the saved options, so the next launch does not send.",
        0,
        1,
    ),
    (4, "FlightRecorder_Mark", "FlightRecorder_Mark( label )", "Write a mark with the current context.", 1, 1),
    (5, "FlightRecorder_Status", "FlightRecorder_Status", "Recorder status as JSON.", 0, 0),
    (
        6,
        "FlightRecorder_SimulateFreeze",
        "FlightRecorder_SimulateFreeze( milliseconds )",
        "Block FileMaker's main thread (test only, max 10000 ms).",
        1,
        1,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_offsets_match_header() {
        let s = core::mem::MaybeUninit::<ExternCallStruct>::uninit();
        let base = s.as_ptr() as usize;
        let off = |p: usize| p - base;
        unsafe {
            let p = s.as_ptr();
            assert_eq!(off(core::ptr::addr_of!((*p).unused_id) as usize), 2);
            assert_eq!(off(core::ptr::addr_of!((*p).entry_point) as usize), 6);
            assert_eq!(off(core::ptr::addr_of!((*p).which_call) as usize), 15);
            assert_eq!(off(core::ptr::addr_of!((*p).unsafe_calls) as usize), 16);
            assert_eq!(off(core::ptr::addr_of!((*p).parm1) as usize), 17);
            assert_eq!(off(core::ptr::addr_of!((*p).parm2) as usize), 18);
            assert_eq!(off(core::ptr::addr_of!((*p).result) as usize), 42);
            assert_eq!(off(core::ptr::addr_of!((*p).c_current_env) as usize), 66);
        }
    }

    #[test]
    fn options() {
        assert_eq!(options_string(false, true, true, false), "SCFR1nnYYnn");
        assert!(PLUGIN_ID[0] != b'F' && PLUGIN_ID.iter().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(get_string(K_OPTIONS_STR).len(), 11);
        assert_eq!(get_string(K_NAME_STR), "FlightRecorder");
    }

    #[test]
    fn utf16_truncates_and_terminates() {
        let mut b = [0xFFFFu16; 6];
        let n = unsafe { write_utf16(b.as_mut_ptr(), b.len(), "Größe!!") };
        assert_eq!(n, 5);
        assert_eq!(b[5], 0);
        assert_eq!(String::from_utf16_lossy(&b[..5]), "Größe");
        let back = unsafe { read_utf16z(b.as_ptr()) };
        assert_eq!(back, "Größe");
    }

    #[test]
    fn function_table_is_sane() {
        let mut ids: Vec<_> = FUNCTIONS.iter().map(|f| f.0).collect();
        ids.dedup();
        assert_eq!(ids.len(), FUNCTIONS.len());
        assert!(FUNCTIONS
            .iter()
            .all(|f| f.1.starts_with("FlightRecorder_") && f.4 <= f.5));
    }
}
