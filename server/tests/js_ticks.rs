//! Runs the browser-free JS tests (tests/js/*.test.mjs) with Node. Skipped
//! when `node` is not installed.

use std::path::Path;
use std::process::Command;

#[test]
fn tick_planner_never_loops_on_a_bad_range() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/js");
    let file = dir.join("ticks.test.mjs");
    let out = match Command::new("node").arg(&file).output() {
        Ok(o) => o,
        Err(_) => {
            eprintln!("node not found: JS test skipped");
            return;
        }
    };
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
