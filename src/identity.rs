//! Who is sending: install id, session id, computer name, OS version.
//!
//! Everything here can touch the disk or ask the system, so it runs on the
//! writer thread, never on FileMaker's main thread. Only [`random_id`] (one
//! `getentropy` call) is used on the main thread.

use crate::wire::ClientInfo;
use std::fs;
use std::path::{Path, PathBuf};

/// 32 random hex characters (128 bits).
pub fn random_id() -> String {
    let mut b = [0u8; 16];
    // getentropy: no file descriptor, cannot block once the system is up.
    let ok = unsafe { libc::getentropy(b.as_mut_ptr().cast(), b.len()) } == 0;
    if !ok {
        // Very unlikely. Mix time and pid so ids still differ.
        let t = crate::eventlog::unix_ms_now() as u128;
        let x =
            (t << 32) ^ ((std::process::id() as u128) << 96) ^ crate::recorder::mono_ns() as u128;
        b = x.to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn is_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|c| c.is_ascii_hexdigit())
}

/// Default folder for the install id (and other plug-in state):
/// $FLIGHT_RECORDER_STATE_DIR, else ~/Library/Application Support/FlightRecorder.
pub fn default_state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("FLIGHT_RECORDER_STATE_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library/Application Support/FlightRecorder")
}

/// Read `<dir>/install-id`, or make it once. If the disk fails, the id is
/// random for this session only, and the error is returned too.
pub fn install_id(dir: &Path) -> (String, Option<String>) {
    let path = dir.join("install-id");
    if let Ok(s) = fs::read_to_string(&path) {
        let s = s.trim();
        if is_id(s) {
            return (s.to_ascii_lowercase(), None);
        }
    }
    let id = random_id();
    let write = || -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("install-id.{}.tmp", std::process::id()));
        fs::write(&tmp, format!("{id}\n"))?;
        fs::rename(&tmp, &path)
    };
    match write() {
        Ok(()) => (id, None),
        Err(e) => (id, Some(format!("install id {}: {e}", path.display()))),
    }
}

/// Host name, e.g. "Front-Desk-iMac.local".
pub fn host_name() -> String {
    let mut buf = [0u8; 256];
    let r = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if r != 0 {
        return String::new();
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// "macOS 26.0.1".
pub fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        let mut buf = [0u8; 64];
        let mut len = buf.len();
        let r = unsafe {
            libc::sysctlbyname(
                c"kern.osproductversion".as_ptr(),
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if r == 0 {
            let n = buf[..len].iter().position(|&c| c == 0).unwrap_or(len);
            return format!("macOS {}", String::from_utf8_lossy(&buf[..n]));
        }
        "macOS".into()
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::consts::OS.into()
    }
}

/// The macOS computer name (System Settings > General > Sharing), or the
/// host name if it cannot be read.
pub fn computer_name() -> String {
    #[cfg(target_os = "macos")]
    if let Some(n) = mac::computer_name() {
        if !n.trim().is_empty() {
            return n;
        }
    }
    host_name()
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

    #[link(name = "SystemConfiguration", kind = "framework")]
    extern "C" {
        fn SCDynamicStoreCopyComputerName(store: *const c_void, enc: *mut u32) -> CFStringRef;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringGetLength(s: CFStringRef) -> isize;
        fn CFStringGetCString(s: CFStringRef, buf: *mut u8, size: isize, enc: u32) -> u8;
        fn CFRelease(p: *const c_void);
    }

    pub fn computer_name() -> Option<String> {
        unsafe {
            let s = SCDynamicStoreCopyComputerName(std::ptr::null(), std::ptr::null_mut());
            if s.is_null() {
                return None;
            }
            // UTF-16 units -> at most 4 UTF-8 bytes each, plus NUL.
            let cap = CFStringGetLength(s).max(0) * 4 + 1;
            let mut buf = vec![0u8; cap as usize];
            let ok = CFStringGetCString(s, buf.as_mut_ptr(), cap, UTF8) != 0;
            CFRelease(s);
            if !ok {
                return None;
            }
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf8_lossy(&buf[..n]).into_owned())
        }
    }
}

/// Everything the server needs to know who sends. `client` is the
/// `client=` option (empty = computer name). Errors are returned as text.
pub fn resolve(state_dir: &Path, client: &str, app: &str) -> (ClientInfo, Option<String>) {
    let (install_id, err) = install_id(state_dir);
    let info = ClientInfo {
        install_id,
        name: if client.is_empty() {
            computer_name()
        } else {
            client.to_string()
        },
        machine: host_name(),
        os: os_version(),
        app: app.to_string(),
        plugin: crate::VERSION.into(),
    };
    (info, err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_ids_are_hex_and_differ() {
        let (a, b) = (random_id(), random_id());
        assert!(is_id(&a) && is_id(&b), "{a} {b}");
        assert_ne!(a, b);
    }

    #[test]
    fn install_id_is_made_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("fr-id-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (a, e) = install_id(&dir);
        assert!(e.is_none(), "{e:?}");
        let (b, _) = install_id(&dir);
        assert_eq!(a, b);
        // A damaged file is replaced.
        fs::write(dir.join("install-id"), "garbage").unwrap();
        let (c, _) = install_id(&dir);
        assert!(is_id(&c) && c != a);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_id_survives_a_bad_folder() {
        // A file where the folder should be: no id on disk, but still an id.
        let f = std::env::temp_dir().join(format!("fr-id-file-{}", std::process::id()));
        fs::write(&f, "x").unwrap();
        let (id, err) = install_id(&f.join("sub"));
        assert!(is_id(&id));
        assert!(err.is_some());
        let _ = fs::remove_file(&f);
    }

    #[test]
    fn system_names() {
        assert!(!computer_name().is_empty());
        assert!(!host_name().is_empty());
        let os = os_version();
        #[cfg(target_os = "macos")]
        assert!(os.starts_with("macOS ") && os.len() > 6, "{os}");
        let dir = std::env::temp_dir().join(format!("fr-id-r-{}", std::process::id()));
        let (c, _) = resolve(&dir, "Desk 4", "Pro 26");
        assert_eq!((c.name.as_str(), c.app.as_str()), ("Desk 4", "Pro 26"));
        let (c2, _) = resolve(&dir, "", "Pro 26");
        assert_eq!(c2.name, computer_name());
        assert_eq!(c.install_id, c2.install_id);
        let _ = fs::remove_dir_all(&dir);
    }
}
