# FileMaker plug-in SDK notes (for the Flight Recorder prototype)

Written 2026-11-08 on an arm64 Mac with FileMaker Pro 26.0.3 installed. "Checked here" means
I checked it on this Mac (files, headers, binaries), not by running FileMaker. "Unconfirmed"
means no source says so and it needs the test plan in [design.md](design.md#test-plan-for-the-three-feasibility-questions).
Later, the plug-in loaded and recorded freezes in FileMaker Pro 26.0.3 on macOS (see the
Status section in [README.md](../README.md#status)). These notes do not mark items as
confirmed from that run.

## 1. Where the SDK comes from, and what is on this Mac

- **Download.** The Claris downloads page links "Plug-in SDK for Windows, Mac, Linux, and
  iOS": `https://downloads.claris.com/DEVREL/sdk/fm_plugin_sdk_26.0.3.307.zip`
  (https://www.claris.com/resources/downloads/).
  - **Checked here:** it downloads with **no login**: HTTP 200, 4,230,493 bytes,
    sha256 `f9bf7eb31b7fe0c063eab43a6e5ca4c709b91315bf1b76cdfde50342ea0b9d62`.
- **Contents of the zip** (`PlugInSDK/`):
  - `Headers/FMWrapper/*.h`: FMXExtern, FMXCalcEngine, FMXText, FMXData, FMXTypes,
    FMXBinaryData, FMXDateTime, FMXFixPt, FMXTextStyle, FMXClient, FMWrapper.
  - `Libraries/Mac/FMWrapper.framework`, plus Win/Linux/iOS libraries.
  - `MiniExample/`: a C++ sample with an Xcode project, CMake and a Visual Studio solution.
  - `README.txt`: sample build environments, links on signing, script step XML.
- **License.** The header grants a limited license "solely to enable licensees of Claris
  FileMaker Pro to compile plug-ins". So the SDK is **not committed** here. It sits in `sdk/`
  (gitignored), and [design.md](design.md#build) says how to fetch it.
  (https://docs.rs/crate/fm_plugin/0.1.17/source/headers/FMXExtern.h)
- **Before the download, no SDK headers were on this Mac** (checked here: `find /` for
  `FMXExtern.h`). But FileMaker Pro.app ships
  `Contents/Frameworks/FMWrapper.framework`, universal x86_64 + arm64, version 26.0.3.307.
  - It exports the whole C API, about 250 `FM_*` symbols such as `FM_ExprEnv_Evaluate`,
    `FM_ExprEnv_EvaluateGetFunction`, `FM_ExprEnv_ExecuteFileSQL`,
    `FM_ExprEnv_RegisterExternalFunctionEx`, `FM_Text_*` and `FM_Data_*` (`nm -gU`).
  - So a plug-in can link without the SDK; only the headers are missing. `build.rs` falls
    back to this copy.
- **Headers vs. older copies.** The 26.0.3 headers were compared with the FM19-era copies
  bundled in the `fm_plugin` crate:
  - the `FMX_ExternCallStruct` layout is unchanged
  - `kCurrentExtnVersion` is **77** (`k260ExtnVersion`); it was 62 in FM19 and 71 in 2022
  - the `unsafeCalls` comment now reads **"True if not on the main thread"**
  - `cfmCalls` is renamed `unusedFlag`
  - `scripts/check-abi.sh` static_asserts our Rust mirror against the SDK headers for
    arm64 and x86_64. It passes.

## 2. Building and loading on macOS

- **Bundle.** The extension must be `.fmplugin` (`.fmx64` on Windows, `.fmx` on Linux)
  (https://help.claris.com/en/pro-help/content/install-plug-in-file.html).
  - The SDK sample's `Info.plist` (checked here) uses `CFBundlePackageType = FMXT`,
    `CFBundleSignature = <4-char plug-in id>`, and `CFBundleExecutable`.
  - The Xcode project builds with `WRAPPER_EXTENSION = fmplugin`, `-weak_framework FMWrapper`
    and `LD_RUNPATH_SEARCH_PATHS = @loader_path/../Frameworks`.
  - We use the same keys, with strong linking and an extra rpath
    `@executable_path/../Frameworks`, which is where FileMaker's own FMWrapper lives.
- **Version string.** `Get(InstalledFMPlugins)` reports `CFBundleVersion` (SDK README).
- **Entry point.** `extern void FMExternCallProc(FMX_ExternCallPtr pb)` is exported by name.
  The sample also defines a global `gFMX_ExternCallPtr` and sets it on every call.
  - Our Rust: `#[no_mangle] pub unsafe extern "C" fn FMExternCallProc`. `build-plugin.sh`
    checks the export with `nm`.
- **Install location** (https://help.claris.com/en/pro-help/content/installing-plug-ins.html):
  - `~/Library/Application Support/FileMaker/Extensions/` for all versions, or
  - `~/Library/Application Support/FileMaker/FileMaker Pro/<ver>/Extensions/`. On this Mac
    `<ver>` is `26.0` (checked here: the folder exists, with no Extensions subfolder yet).
  - Duplicates in several folders can make an old copy load
    (https://www.mbsplugins.de/archive/2022-04-13/Why_MBS_Plugin_fails_to_load).
  - Enable it under FileMaker Pro > Settings > Plug-Ins
    (https://help.claris.com/archive/fm21/en/pro-help/content/plug-in-preferences.html).
- **Signing and notarization** for a local test:
  - FileMaker Pro does **not** require a signature. If it cannot verify one, it asks. The
    user ticks "Always load this plug-in" and clicks Load Plug-in, and the plug-in goes on
    the Settings > Permitted list
    (https://help.claris.com/en/pro-help/content/permitted-hosts-preferences.html).
  - BaseElements says a developer certificate is not needed "for compiling and testing,
    provided you authorize the plugin"
    (https://raw.githubusercontent.com/GoyaPtyLtd/BaseElements-Plugin/main/BUILDING.md).
  - **Checked here:** FileMaker Pro.app has the `com.apple.security.cs.disable-library-validation`
    entitlement, so the hardened runtime lets it load our ad-hoc-signed library.
  - A bundle built on this Mac has no `com.apple.quarantine` attribute, so Gatekeeper is not
    involved. A copy downloaded or unzipped from elsewhere needs
    `xattr -dr com.apple.quarantine`.
  - Do not modify a signed bundle afterwards; `lipo` or sync services break the signature
    (MBS links above).
  - Notarization matters only for distribution (MBS notarizes "to avoid friction":
    https://mbsplugins.de/archive/2019-07-25/Notarized_MBS_FileMaker_Plugin). Whether
    FileMaker checks notarization itself is unconfirmed.
  - The SDK README points to https://www.troi.com/plug-in-development/code-signing-filemaker-plugins/
    and http://fmplugins.idma.nz/index.php?title=Code_Signing.
- **Apple Silicon.** "By default only plug-in … versions built for Apple silicon work".
  Intel-only plug-ins need FileMaker under Rosetta
  (https://help.claris.com/en/pro-installation-guide/content/running-pro-on-apple-silicon.html).
  We build universal (arm64 + x86_64) with `lipo` *before* signing.

## 3. Entry point and messages (from FMXExtern.h, SDK 26.0.3)

`FMX_ExternCallStruct`, packed (`__attribute__((packed))` on clang), 74 bytes on LP64:

| offset | field | notes |
|---|---|---|
| 0 | `int16 extnVersion` | API version of the host |
| 2 | `int32 unusedID` | always -1 since FM19 |
| 6 | `entryPoint` | |
| 14 | `unusedFlag` (was `cfmCalls`) | 0 |
| 15 | `uchar whichCall` | the message |
| 16 | `unsafeCalls` | **"True if not on the main thread"** |
| 17 | `uchar parm1` | |
| 18, 26 | `ptrtype parm2, parm3` | |
| 34 | `instanceID` | Mac: the CFBundleRef |
| 42 | `result` | |
| 50 | `unused` | |
| 58 | `cStartScript` | `FMX_StartScript(file, script, control, param)` |
| 66 | `cCurrentEnv` | `FMX_SetToCurrentEnv(ExprEnv*)` |

Messages (`whichCall`) and their parameters:

| message | value | parm1 | parm2 | parm3 / result |
|---|---|---|---|---|
| `kFMXT_Init` | 0 | app type | app version, UTF-16 C string | result = API version, or `kDoNotEnable` (-2) |
| `kFMXT_Idle` | 1 | idle level | session id | |
| `kFMXT_Shutdown` | 4 | | | |
| `kFMXT_DoAppPreferences` | 5 | | | |
| `kFMXT_GetString` | 7 (must be handled) | string id: Name 128, AppConfig 129, Options 131, HelpURL 132 | Windows language id | parm3 = buffer size in UTF-16 units; result = buffer pointer |
| `kFMXT_SessionShutdown` | 8 | | session | |
| `kFMXT_FileShutdown` | 9 | | session | file |

**Options string** (11 characters):

| position | meaning |
|---|---|
| 1–4 | plug-in id |
| 5 | `1` |
| 6 | `Y` = has a preferences button |
| 7 | `n` |
| 8 | `Y` = send Init/Shutdown |
| 9 | `Y` = send Idle |
| 10 | `Y` = send Session/File shutdown |
| 11 | `n` |

We send `SCFR1nnYYnn`. Plug-in ids must be unique low-ASCII alphanumerics and **must not start
with "F", "FM" or "Web"** (https://homeappliance.manualsonline.com/manuals/mfg/filemaker/8.html?p=49).
Our first choice, `FLRC`, broke that rule, so the prototype uses `SCFR`. It is not registered
with Claris.

**Idle levels** (`parm1` of `kFMXT_Idle`):

| constant | value | header comment |
|---|---|---|
| `kFMXT_UserIdle` | 0 | "user hasn't done anything for 30 seconds or more" |
| `kFMXT_UserNotIdle` | 1 | "has done something within 30 seconds" |
| `kFMXT_ScriptPaused` | 2 | |
| `kFMXT_ScriptRunning` | 3 | |
| `kFMXT_Unsafe` | 4 | "same as if unsafeCalls is set to true" |

- The SDK sample's `Do_PluginIdle` switches on all five but has an empty body.
- How often idle is sent, and whether it is sent during a long layout draw or calc, is not
  documented (**unconfirmed**). The recorder records what it sees (`heartbeats` in
  `session_stop`), and the test plan in [design.md](design.md#test-plan-for-the-three-feasibility-questions) measures it.

**External functions.** Registered with:

```
FM_ExprEnv_RegisterExternalFunctionEx(const QuadChar& id, short funcId, const Text& name,
    const Text& prototype, const Text& description, short min, short max, uint32 flags,
    ExtPluginType fn, _fmxcpt& x)
```

- The callback signature is `errcode fn(short funcId, const ExprEnv&, const DataVect& parms, Data& result)`.
  References are pointers at the ABI level.
- Flags: `kDisplayInAllDialogs = 0xFF00`, `kMacCompatible = 0x02` (`kWin 0x04`,
  `kServer 0x08`, `kIOS 0x10`, `kCustomWeb 0x20`, `kWebDirect 0x40`).
- Every C export takes a trailing `_fmxcpt&` = `{int32 m_vers = 1; int32 m_code = 0}`. FileMaker
  fills it when the C++ side throws.

## 4. What can be evaluated during idle

- **Getting an `ExprEnv` during idle.** `FM_ExprEnv_Constructor1`, then `cCurrentEnv(env)`
  (`FMX_SetToCurrentEnv`) binds it to the current window's file.
- **`Get()` values.** `FM_ExprEnv_EvaluateGetFunction(env, kGet_*, data)` reads them without
  parsing a calculation. IDs: FileName 1005, HostName 1008, LayoutName 1010, WindowMode 1013,
  ScriptName 1026, UserName 1031, AccountName 1032, WindowName 1048, LayoutTableName 1061
  (FMXCalcEngine.h, API 60+). `Evaluate(text)` and `ExecuteFileSQL` / `ExecuteFileSQLTextResult`
  are also available on the same env.
- **Idle-time `Evaluate` is used in practice.** MBS offers `Schedule.EvaluateOnIdle`
  (https://mbsplugins.eu/ScheduleEvaluateOnIdle.shtml). The exact per-level rules are
  **unconfirmed**. We only call FileMaker when `unsafeCalls == 0` and the level is 0, 1 or 2.
  Level 3 (script running) is opt-in (`scripts=1`).
- **Server differences.** On FileMaker Server, `FMX_SetToCurrentEnv` and `FMX_StartScript` do
  not work, and idle is sent only while a script is paused
  (https://mbsplugins.de/archive/2015-08-02/FileMaker_Plugin_development_t).
  That does not matter here: this is a client plug-in.

## 5. Threading rules

- **Main thread only.** Call FMWrapper only on the thread FileMaker called you on, its main
  thread. A plug-in calling back from another thread gets "FileMaker called plugin on thread:
  Thread[main…] but plugin is trying to call back to FileMaker on a different thread … This is
  not allowed" (https://fmforums.com/topic/82657-error-with-thread-on-sm-module). The SDK 26
  comment on `unsafeCalls` ("True if not on the main thread") points the same way.
- **What our threads do.** The watchdog, the network sampler and the log writer never touch
  FMWrapper. They read atomics, call `getsockopt`, and write files.
- **Consequence.** The context (layout, script) **cannot be read during a freeze**. The main
  thread is the frozen one, and no other thread may ask the calc engine. We read it at the
  last safe idle before the freeze and the first one after it.

## 6. Existing work

**Rust:** the `fm_plugin` crate, https://github.com/mutantcows/rust_fm_plugin.
- MIT; last release 0.1.17 on 2021-04-03; tested on FM 18/19; effectively unmaintained.
- It vendors the FM19 headers and libraries, `bindgen` output, and a `register_plugin!` macro.
- We did **not** depend on it. It is old, pulls a large vendored SDK, and we needed only 20
  functions. We used its source and headers as a cross-check (`docs.rs/crate/fm_plugin`).
- No other Rust bindings found.

**C++ starting points:**
- BaseElements (open source; https://github.com/GoyaPtyLtd/BaseElements-Plugin)
- SimplePlugin (https://www.github.com/minstral/SimplePlugin)
- libfmplug (https://github.com/tibogens/libfmplug)
- the SDK's MiniExample
- Commercial: MBS, 360Works, Troi.

**Why Rust and not C++.** The SDK does not force C++. All FMWrapper entry points are
`extern "C"` with opaque pointers, so the C++ classes in the headers are thin inline
wrappers. Rust calls the C exports directly. The packed struct and the `_fmxcpt` convention
are the only ABI details, and both are checked by `scripts/check-abi.sh`.

**Client-side monitoring today:**

| tool | what it gives | gap |
|---|---|---|
| MBS Trace (`Trace.SetWithTimes`) (https://www.mbsplugins.eu/Trace.shtml) | logs MBS plug-in calls with timings | not general FileMaker activity |
| 24U FM Bench / Detective (https://fmforums.com/topic/81507-24u-fm-bench-detective-102-released) | script and calc timing | FM 11–13 era |
| FileMaker Pro built-ins | `Set Error Logging` (FM18+) writes script errors to a file (https://dbservices.com/blog/filemaker-script-error-logging); the Script Debugger | interactive only; no freeze log |
| FileMaker Server `TopCallStats.log` (https://help.claris.com/en/server-help/content/monitor-top-call-log.html) | per call: elapsed, wait, I/O, network bytes, target file::table, client name | server side only |
| FileMaker Server `ClientStats.log` (https://help.claris.com/en/server-help/content/monitor-client-stats-log.html) | per client per interval: Remote Calls, bytes, wait | server side only; a good cross-check for our round-trip counts |
| Server log tools (Zabbix templates, OttoFMS, FMS Detective) | parse the server logs | server side only |

I found nothing that records **client** freezes with the layout, script and round trips. No
public product named "FM Perflyzer" or "fmPerception" was found.

## 7. Feasibility answers

### (a) Can a freeze be detected?

**Yes. Use a watchdog thread plus a run-loop observer; idle gaps alone are not enough.**

- **Idle gaps alone are ambiguous.** FileMaker idle messages arrive at an undocumented
  cadence. A gap can mean "frozen" or "nothing to do". Treating every gap above 250 ms as a
  freeze would log the user's coffee breaks.
- **The robust method:**
  1. FileMaker's main thread publishes a heartbeat: an atomic store, 26 ns, measured with
     `examples/bench_probe`. It does this on every `kFMXT_Idle` *and* on every run-loop
     activity, through a `CFRunLoopObserver` added to the main run loop in common modes.
  2. The observer also reports `kCFRunLoopBeforeWaiting` (going to sleep, so not frozen) and
     `AfterWaiting` (woke up).
  3. A watchdog thread polls every 10 ms. "Awake and no heartbeat for ≥ threshold" is a
     freeze. The freeze ends at the next heartbeat or when the run loop sleeps.

  This is the standard hang-detector method on Apple platforms. It needs no FileMaker API on
  the watchdog thread.
- **Tested** with a real CFRunLoop on the main thread: `examples/simulate_host` and the unit
  test in `src/runloop.rs`. Freeze durations are within about 10 ms of the injected work.
- **Limits:**
  - A long operation that pumps a nested run loop (for example a script with a progress
    window, or Allow User Abort on) is *not* a frozen UI, so it is not logged as a freeze.
    Long "script running" periods are logged as `script_busy` instead.
  - We do not sample the main thread's stack. That would need Mach `thread_get_state` and
    symbolication; possible later.

### (b) Can a freeze be tied to layout, script and user without slowing FileMaker?

**Yes, with one limit: the context comes from just before and just after the freeze, never
from inside it.**

- **How.** At a safe idle (`unsafeCalls == 0`, level 0, 1 or 2), at most every 250 ms and
  right after each freeze, we build an `ExprEnv`, bind it with `cCurrentEnv` and read 9
  `Get()` values with `EvaluateGetFunction`. No calculation parsing is involved.
- **Cost.** The capture time is logged with every context (`capture_us`). Measuring it inside
  FileMaker is test (b) in [design.md](design.md#test-plan-for-the-three-feasibility-questions). The freeze detection itself never calls FileMaker.
- **The limit.** A trigger that freezes the UI right away (OnRecordLoad, OnLayoutEnter) may
  not appear as `Get(ScriptName)` in either context, because no safe idle happens while it
  runs. The simulator shows this case. Mitigations:
  - `FlightRecorder_Mark("…")` inside slow scripts. Marks carry their own context, read with
    the script's own `ExprEnv`, and are attached to freezes within 1 s.
  - `FlightRecorder_Start("scripts=1")` also reads context at idle level 3. Whether FileMaker
    allows that safely is **unconfirmed**; test it.
- **User.** `Get(AccountName)` and `Get(UserName)`.

### (c) Can round trips to the server be counted from the client?

**Yes, from inside the process, at about 0.06% of one core.**

- **How:**
  1. The plug-in shares FileMaker's file descriptor table. Every 200 ms until found, then
     every 2 s, it lists its own fds (`proc_pidinfo(PROC_PIDLISTFDS)`) and keeps the TCP
     sockets whose `getpeername` port is 5003.
  2. Every 2 ms it reads `getsockopt(fd, IPPROTO_TCP, TCP_CONNECTION_INFO)` from the public
     `<netinet/tcp.h>`: bytes and packets sent and received, smoothed RTT.
  3. A round trip is counted when sent bytes grow and then received bytes grow. The
     request→reply time is summed as `wait_ms`: network plus server work plus waiting.

  Payloads are never read (they are TLS anyway).
- **Cost** (`examples/bench_probe`, M-series Mac):

  | operation | cost | frequency |
  |---|---|---|
  | one sample, 4 sockets | 1.2 µs | every 2 ms (0.06% of one core) |
  | full fd scan, about 90 fds | 27 µs | every 2 s |
  | main-thread heartbeat | 26 ns | per heartbeat |
- **Accuracy:**
  - **Simulator:** with a fake server on real sockets the counts were exact: 30/30, 3/3,
    60/60 and 1/1. `wait_ms` separated a 2 s lock wait (2001 of 2001 ms) from a CPU-bound
    freeze (9 of 708 ms).
  - **Lower bound by design.** Round trips that finish inside one 2 ms sample merge. A
    sub-millisecond LAN can undercount; a WAN, where it hurts, is counted well. If FileMaker
    pipelines requests (several outstanding at once), they count as one.
  - **Why not just use RTT.** The kernel's RTT measures ACKs, not server answers.
    "Round trips × RTT" missed a 2 s lock entirely, which is why `wait_ms` exists.
  - **Unconfirmed:** whether FileMaker Pro uses one TCP connection per host or per file, and
    whether "Use HTTPS tunneling" moves traffic to 443 (use `port=443` then).
- **Alternatives we did not use:**
  - `nettop`: a private tool; spawning it in a loop costs a lot of CPU
    (https://developer.apple.com/forums/thread/757294).
  - NetworkStatistics.framework: private.
  - libproc `tcp_sockinfo` from outside the process: it has ports and state but no byte
    counters
    (https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/sys/proc_info.h).
  - A packet capture: needs root.
- **Cross-check.** FileMaker Server's ClientStats.log reports "Remote Calls" per client per
  interval.
