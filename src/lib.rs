//! FileMaker Flight Recorder – core.
//!
//! Records client-side freezes in FileMaker Pro: when, how long, where
//! (file, layout, script, account) and how many round trips to FileMaker
//! Server happened during the freeze. See https://github.com/solutionscay/fm-flight-recorder/wiki/Design
//! and https://github.com/solutionscay/fm-flight-recorder/wiki/SDK-notes.
//!
//! The core (everything except `fmx`) has no FileMaker dependency and is
//! tested with `cargo test`. The FileMaker glue is behind the `fmx` feature.

pub mod context;
pub mod detector;
pub mod eventlog;
pub mod fmxabi;
pub mod http;
pub mod identity;
#[cfg(target_os = "macos")]
pub mod machthread;
pub mod net;
pub mod recorder;
#[cfg(target_os = "macos")]
pub mod runloop;
pub mod sender;
pub mod spool;
pub mod startopts;
pub mod wire;

#[cfg(feature = "fmx")]
pub mod fmx;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A probe for the configured port, or a no-op probe.
pub fn default_probe(port: Option<u16>) -> Box<dyn net::NetProbe> {
    match port {
        #[cfg(target_os = "macos")]
        Some(p) => Box::new(net::TcpPortProbe::new(p)),
        _ => Box::new(net::NoProbe),
    }
}
