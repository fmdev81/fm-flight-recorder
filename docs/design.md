# FileMaker Flight Recorder (prototype)

A FileMaker Pro client plug-in that records **client-side freezes**: when FileMaker stopped
responding, for how long, where the user was (file, layout, script, account), and how many
network round trips to FileMaker Server happened during the freeze. It also records how much of
the freeze was spent waiting for the server.

FileMaker Server's Top Call Stats and logs show calls from the server's side. They do not show
what the user waits through on the client:
- a layout that draws unstored calculations
- a portal that makes 200 round trips
- a trigger loop
- a commit that waits on the server

This plug-in records those.

```
16:44:55  froze 0.9 s on Invoices | Detail (from Dashboard), script Go To Invoices, 30 round trips, 0.9 s waiting on server
16:44:57  froze 0.7 s on Customers | List (from Invoices | Detail), 3 round trips, 0.0 s waiting on server
16:45:01  froze 2.0 s on Customers | List, 1 round trip, 2.0 s waiting on server
```

Status: **prototype, not yet run inside FileMaker Pro.**
- What is done:
  - The core is unit-tested and end-to-end tested with real sockets and a real macOS run loop.
  - The plug-in links against the official Claris SDK 26.0.3 `FMWrapper.framework`.
  - The ABI is checked against the SDK headers.
- What is not done: loading it in FileMaker is the first step of the test plan below.

Research, sources and the three feasibility answers: [docs/sdk-notes.md](sdk-notes.md).

## How it works

```
FileMaker main thread                       plug-in threads (never call FileMaker)
─────────────────────                       ──────────────────────────────────────
kFMXT_Idle ──► beat()  (atomic, 26 ns)      fr-watchdog: every 10 ms
CFRunLoopObserver ──► beat()/sleeping         "awake and silent ≥ 250 ms" = freeze
safe idle, ≤ every 250 ms:                  fr-net: every 2 ms getsockopt(TCP_CONNECTION_INFO)
  Get(LayoutName…) ──► context                on FileMaker's own sockets to port 5003:
FlightRecorder_Mark(label) ──► mark           round trips + request→reply wait
                                            fr-writer: JSONL → ~/Library/Logs/FlightRecorder/
```

- **Freeze.** The main run loop is awake and has not come back for at least `threshold` ms
  (default 250). If the run loop is asleep, FileMaker is idle, not frozen.
- **Context.** Read on the main thread at the last safe idle before the freeze (`before`) and
  the first one after it (`after`). It cannot be read *during* a freeze, because only the frozen
  thread may call FileMaker.
- **Round trips.** Counted from the TCP counters of FileMaker's connection to port 5003, inside
  the process. A round trip is "client sent, then server answered". `wait_ms` is the summed
  request→reply time (network plus server). Payloads are never read.
- **Script busy.** FileMaker reports "script running" for longer than the threshold while the UI
  still pumps events. This is logged as `script_busy`.

## Repository layout

| Path | What |
|---|---|
| `src/detector.rs` | Stall detector, a pure state machine fed with heartbeat samples |
| `src/context.rs` | Context struct, `Get()` capture through a trait, rate gate |
| `src/net.rs` | Turn counter, net history, macOS in-process TCP probe |
| `src/eventlog.rs` | JSONL schema and writer |
| `src/recorder.rs` | Threads, the main-thread API, options |
| `src/wire.rs` | Wire types of the ingest API ([ingest-api.md](ingest-api.md)) |
| `src/identity.rs` | Install id (kept in `~/Library/Application Support/FlightRecorder/install-id`), session id, computer name, OS version |
| `src/runloop.rs` | `CFRunLoopObserver` heartbeat (plus timer helpers for the simulator) |
| `src/fmxabi.rs` | FMX ABI facts (struct layout, constants, options string), tested without FileMaker |
| `src/fmx/` | The plug-in glue (feature `fmx`): `FMExternCallProc`, external functions, FFI to FMWrapper |
| `examples/simulate_host.rs` | Drives the real recorder like FileMaker would, with a fake server |
| `examples/bench_probe.rs` | Cost of the heartbeat and of a probe sample |
| `src/bin/fakelog.rs` | `fr-fakelog`: writes a realistic fake log for demos |
| `viewer/index.html` | Static viewer. `viewer/demo.html` is the same viewer with the fake log embedded. |
| `scripts/` | `build-plugin.sh`, `install-plugin.sh`, `check-abi.sh`, `summary.py`, `make-demo-html.py` |

## Build

Requirements: Rust (stable, tested with 1.90), Xcode command line tools, and macOS on Apple Silicon or Intel.

```sh
cargo test                          # 30 unit + 5 end-to-end tests; FileMaker not needed

# Get the Claris SDK (public download, no login). It is not committed: its license allows
# use for compiling plug-ins only.
mkdir -p sdk && cd sdk
curl -LO https://downloads.claris.com/DEVREL/sdk/fm_plugin_sdk_26.0.3.307.zip
unzip -q fm_plugin_sdk_26.0.3.307.zip && cd ..
# Without sdk/, the build links against /Applications/FileMaker Pro.app's own FMWrapper.framework.

scripts/check-abi.sh                # our Rust struct and constants vs. the SDK headers
scripts/build-plugin.sh             # → dist/FlightRecorder.fmplugin (universal, ad-hoc signed)
```

To get the x86_64 slice: `rustup target add x86_64-apple-darwin`. Without it, the bundle is
arm64 only.

For a Developer ID signature:

```sh
SIGN_IDENTITY="Developer ID Application: …" scripts/build-plugin.sh
```

You do not need one on your own Mac.

### Demo without FileMaker

```sh
cargo run --example simulate_host -- demo-logs     # real recorder, fake server, 4 freezes
python3 scripts/summary.py demo-logs/*.jsonl
open viewer/demo.html                               # viewer with a fake 3-hour log embedded
cargo run --bin fr-fakelog -- --hours 8 --seed 7 --out /tmp/day.jsonl   # another fake log
python3 scripts/make-demo-html.py /tmp/day.jsonl /tmp/day.html && open /tmp/day.html
```

`viewer/index.html` also opens any log by drag-and-drop or with **Open log…**.

## Install and try it in FileMaker Pro

1. **Quit FileMaker Pro.**
2. Build and install:

   ```sh
   scripts/build-plugin.sh
   scripts/install-plugin.sh
   ```

   This copies the bundle to `~/Library/Application Support/FileMaker/Extensions/`. It refuses
   to run while FileMaker is running. It also clears quarantine and verifies the signature.
3. Start FileMaker Pro.
4. **Allow the plug-in.** FileMaker cannot verify an ad-hoc signature, so it shows a dialog.
   Tick **Always load this plug-in**, then click **Load Plug-in**. The decision is stored under
   Settings > Permitted. To undo it, select the entry and press Backspace.
   - **Gatekeeper.** A bundle built on this Mac has no quarantine attribute, so Gatekeeper does
     not run. If you copy the bundle from another Mac or from a zip, run this before step 3:
     ```sh
     xattr -dr com.apple.quarantine ~/Library/Application\ Support/FileMaker/Extensions/FlightRecorder.fmplugin
     ```
5. Open **FileMaker Pro > Settings > Plug-Ins**. Check that **FlightRecorder** is listed and
   ticked.
6. Open any file. In the Data Viewer (or a calculation), evaluate:

   ```
   FlightRecorder_Version            → 0.1.0
   FlightRecorder_Status             → {"running":true,"heartbeats":…,"log":"…/flight-YYYY-MM-DD.jsonl",…}
   FlightRecorder_SimulateFreeze ( 1500 )   → blocks FileMaker for 1.5 s on purpose
   ```

7. Watch the log, or open the viewer:

   ```sh
   tail -f ~/Library/Logs/FlightRecorder/flight-*.jsonl
   python3 scripts/summary.py
   ```

   You should see a `freeze` of about 1500 ms with your layout.

The recorder **starts on its own** when FileMaker launches, with the default options.
- To turn that off: `launchctl setenv FLIGHT_RECORDER_NO_AUTOSTART 1`, then restart FileMaker.
- To uninstall: quit FileMaker and delete the `.fmplugin` from the Extensions folder.

### One-command load test (self-test mode)

```sh
scripts/selftest-in-filemaker.sh /tmp/fr-selftest
```

The script needs FileMaker Pro to be closed. It does the following:

1. Installs the plug-in.
2. Copies FileMaker's own *Tasks* template to `/tmp/fr-selftest/FRSelfTest.fmp12`. It never
   opens your files.
3. Starts FileMaker with `FLIGHT_RECORDER_SELFTEST=1` and opens the copy. In this mode the
   plug-in evaluates calculations through FileMaker's calc engine at idle and logs the results
   as `mark` lines:
   - `FlightRecorder_Version`
   - `Get(...)` values
   - `FlightRecorder_Status`
   - `FlightRecorder_SimulateFreeze(1500)`
   - a `While()` loop
   - `FlightRecorder_Mark`
4. On the **first run only**, FileMaker stops at startup with its signing alert. The script
   waits while you tick *Always load this plug-in* and click *Load Plug-in*.
5. Prints the log lines and quits FileMaker.

Without the environment variable, self-test mode is off.

If FileMaker does not list the plug-in, check:
- `codesign -v --strict` on the installed bundle
- for duplicates in `~/Library/Application Support/FileMaker/FileMaker Pro/26.0/Extensions/`
- `Get ( InstalledFMPlugIns )`

### Functions

| Function | Result |
|---|---|
| `FlightRecorder_Version` | Plug-in version |
| `FlightRecorder_Start ( { options } )` | (Re)starts recording; returns the log path. Options: `threshold=250; port=5003; sample_ms=2; context_ms=250; scripts=0; dir=/path; endpoint=http://host:8787; token=…; client=name`. `port=0` turns off the round-trip counter. Use `port=443` if HTTPS tunnelling is on. `endpoint` and `token` send the events to the companion server ([ingest-api.md](ingest-api.md)); `endpoint` needs `token` and must be `http://`. `client` overrides the client name (default: the macOS computer name). Unknown keys and bad values are listed on a second line of the result, `ignored: foo, port=abc`. A bad `endpoint` or a missing `token` returns `error: …` and leaves the recorder as it was. |
| `FlightRecorder_Stop` | Stops recording; returns `stopped: N freezes, …` |
| `FlightRecorder_Mark ( label )` | Writes a mark with the calling script's context. Marks within 1 s before a freeze are attached to it. A Mark is not a heartbeat. |
| `FlightRecorder_Status` | JSON: running, heartbeats, freezes, current freeze length, last context, log path, endpoint, client (never the token) |
| `FlightRecorder_SimulateFreeze ( ms )` | Blocks the main thread for up to 10 000 ms. For testing only. |

### Log format (`~/Library/Logs/FlightRecorder/flight-YYYY-MM-DD.jsonl`, schema `v: 1`)

```json
{"v":1,"session_id":"b6d0c1e2f3a4958677a8b9c0d1e2f3a4","seq":7,"type":"freeze","ts":"2026-11-08T16:45:01.012Z","end_ts":"2026-11-08T16:45:03.013Z","duration_ms":2001,
 "level":"user_not_idle",
 "before":{"file":"Invoices","layout":"Customers | List","layout_table":"Customers","account":"demo","user":"Demo User","window":"Invoices","host":"…","mode":"0","captured_ms":12345,"capture_us":41},
 "after":{…},
 "net":{"round_trips":1,"tx_bytes":4,"rx_bytes":3000,"tx_packets":1,"rx_packets":2,"srtt_ms":1,"wait_ms":2001,"connected":true},
 "marks":["go to Invoices"]}
```

Every line has:

- `v`: the schema version (1). New optional fields do not change it ([ingest-api.md](ingest-api.md#schema-versions)).
- `session_id`: 32 random hex characters, new at each start of the recorder.
- `seq`: 1, 2, 3, … in the session, in the order the lines are written. (`session_id`, `seq`) is
  the id of the event; the server uses it to drop duplicates.

Lines written before the ingest API have no `session_id` and no `seq`.

Other types:

| Type | Fields |
|---|---|
| `session_start` | pid, app version, plug-in version, threshold, heartbeat sources, port, `client` (the `client=` option, else the macOS computer name), `install_id`, `machine` (host name), `os` |
| `script_busy` | `ts`, `end_ts`, `duration_ms`, `before`, `net` |
| `mark` | `label`, `context` |
| `session_stop` | freezes, heartbeats |

## Test plan for the three feasibility questions

**Sample file `FRTest.fmp12`.** Make it in FileMaker and host it on a FileMaker Server, either
on another machine or on this Mac. It must be hosted: a local file makes no round trips.

Tables:

| Table | Fields | Records |
|---|---|---|
| `Customers` | `id`, `Name`; unstored calc `cBalance = Sum ( Invoices::Total )` | 1 000 |
| `Invoices` | `id`, `CustomerID`, `Total` (number) | 20 000 |
| `LineItems` | `id`, `InvoiceID`, `Qty`, `Price`; unstored calc `cLine = Qty * Price` | 200 000 |

Relationships: Customers::id = Invoices::CustomerID, and Invoices::id = LineItems::InvoiceID.

Layouts:

| Layout | Contents | Simulates |
|---|---|---|
| `Fast` | Form view of Customers::Name only | a layout that should never freeze |
| `Slow List` | List view of Customers showing `cBalance` | unstored calcs, many round trips |
| `Portal` | Invoices form with a 200-row LineItems portal showing `cLine` | a portal fetching rows |
| `Trigger` | Invoices form with an OnRecordLoad script `Refresh` | a trigger script; see below |

The `Refresh` script:

```
FlightRecorder_Mark ( "Refresh start" )
Loop
  20 × ExecuteSQL ( "SELECT SUM(Qty*Price) FROM LineItems WHERE InvoiceID = ?" ; "" ; "" ; Invoices::id )
End Loop
```

Scripts:

| Script | Body | Simulates |
|---|---|---|
| `Busy Script` | Loop 5 000 × Set Variable, with Allow User Abort On | a long script that keeps the UI alive |
| `Server Wait` | `Perform Script on Server [ Wait for completion ]` running `Pause/Resume Script [ 3 s ]` | one round trip, a long server wait |

**(a) Detection.** Run each step a few times and look at the log.

1. Sit on `Fast` for 2 minutes without touching anything. Expect **no** freeze.
   `session_stop`/`Status` shows how many heartbeats arrived. That number, divided by elapsed
   time, is the idle-message rate, which no document gives.
2. `FlightRecorder_SimulateFreeze ( 2000 )`. Expect one freeze of 2000 ± 20 ms.
3. Go to `Slow List` and scroll. Go to `Portal`. Expect freezes whose `duration_ms` matches a
   stopwatch.
4. Run `Busy Script`. Expect `script_busy`, not `freeze`, if FileMaker pumps events during
   scripts. Record which one you get: it answers the question about nested run loops.
5. Open a modal dialog (Show Custom Dialog) and wait 30 s. Expect no freeze.

**(b) Context and cost.**

1. In each freeze from (a), check that `before.layout` and `after.layout` are right, and that
   `account` is your account.
2. For `Trigger`, check whether the script appears in `before.script` or `after.script`. The
   prediction is that it often does not. Then check that the `Refresh start` mark is attached
   (`marks`).
3. Run `FlightRecorder_Start ( "scripts=1" )` and repeat. Does `script` now appear, and is
   FileMaker still stable?
4. Read `capture_us` across the log; the expected range is tens of µs.
5. Slowdown test: time a 10 000-record `Replace Field Contents` with the plug-in enabled, then
   disabled (Settings > Plug-Ins). Expect no measurable difference.

**(c) Round trips.**

1. Enable **Client Statistics** logging on FileMaker Server (Admin Console > Logging > Client
   Statistics, 30 s interval). Compare the server's "Remote Calls" for your client in an
   interval with the sum of `round_trips` in the same interval. Expect ours ≤ the server's
   count (ours is a lower bound) and roughly the same size on a WAN.
2. `Server Wait`: expect one freeze of about 3 s with `round_trips` ≈ 1–3 and `wait_ms` ≈ the
   whole freeze.
3. `Slow List` vs. `Fast`. Expect many round trips on Slow List the first time. On a cached
   revisit, expect fewer round trips with more client-side time.
4. Check `net.connected` is `true`. If it is `false`, check which port FileMaker uses:
   `lsof -nP -p $(pgrep -x "FileMaker Pro") -iTCP`. It may be 443 if HTTPS tunnelling is on.
5. Optional, to test a slow network: add 50 ms latency with the Network Link Conditioner (Xcode
   Additional Tools) and repeat step 3. `wait_ms` should rise sharply.

## Known limits

- **Context comes from around the freeze, not from inside it.** Instant triggers may need
  `FlightRecorder_Mark`.
- **`round_trips` is a lower bound.** Round trips that finish within one 2 ms sample count as
  one, and pipelined requests count as one.
- **macOS only.** On Windows the same design would use `GetPerTcpConnectionEStats` and a
  message-loop hook.
- **The plug-in ID `SCFR` is not registered with Claris.**
- **No main-thread stack sampling yet.** It would show *what* FileMaker was doing (drawing or
  calculating).
