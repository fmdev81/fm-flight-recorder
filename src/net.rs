//! Counting client/server round trips from inside the FileMaker process.
//!
//! A plug-in runs inside FileMaker Pro, so it shares FileMaker's file
//! descriptor table. FileMaker Pro talks to FileMaker Server over TCP port
//! 5003. On macOS, `getsockopt(fd, IPPROTO_TCP, TCP_CONNECTION_INFO)` returns
//! per-connection counters (bytes and packets sent and received, smoothed
//! RTT) for any TCP socket in the process. We:
//!
//! 1. find the sockets whose peer port is 5003 (re-scan every few seconds),
//! 2. sample their counters every few ms on a background thread,
//! 3. count a *turn* (one round trip) each time the client sent data and the
//!    server then answered: tx grows, then rx grows.
//!
//! We never read the payload (it is TLS anyway). The cost is one syscall per
//! connection per sample.
//!
//! Precision: two round trips that finish inside one sample interval look
//! like one. So `turns` is a lower bound. It is exact when the RTT is longer
//! than the sample interval, which is the slow WAN case that matters. The
//! log also keeps the byte and packet deltas.

use serde::{Deserialize, Serialize};

/// Summed counters over all matching connections.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetCounters {
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub connections: u32,
    /// Smoothed RTT of the busiest connection, ms.
    pub srtt_ms: u32,
}

/// Something that can read the current counters.
pub trait NetProbe: Send {
    fn sample(&mut self) -> Option<NetCounters>;
}

/// Pure turn detector. Feed it counter samples in time order.
#[derive(Debug, Clone, Default)]
pub struct TurnCounter {
    last: Option<NetCounters>,
    /// True when the client sent data that has no answer yet.
    awaiting_reply: bool,
    pub turns: u64,
}

impl TurnCounter {
    /// Returns the number of new turns found in this sample (0 or 1).
    pub fn feed(&mut self, c: NetCounters) -> u64 {
        let Some(prev) = self.last.replace(c) else { return 0 };
        // A connection went away or was replaced: counters went down. Restart.
        if c.tx_bytes < prev.tx_bytes || c.rx_bytes < prev.rx_bytes || c.connections != prev.connections {
            self.awaiting_reply = false;
            return 0;
        }
        let sent = c.tx_bytes > prev.tx_bytes;
        let got = c.rx_bytes > prev.rx_bytes;
        let mut new = 0;
        if got && (self.awaiting_reply || sent) {
            // Reply to an earlier request, or request+reply in the same tick.
            new = 1;
            self.awaiting_reply = false;
        }
        if sent && !got {
            // Data sent in this tick and not answered in it starts a new wait.
            self.awaiting_reply = true;
        }
        self.turns += new;
        new
    }
}

/// Network activity inside a time window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetWindow {
    pub round_trips: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub srtt_ms: u32,
    /// False when no FileMaker Server connection was found.
    pub connected: bool,
}

/// Time-stamped history of samples and turns, so a stall that is found
/// late (after `threshold`) can still count what happened from its start.
#[derive(Debug, Default)]
pub struct NetHistory {
    counter: TurnCounter,
    /// (t_ns, cumulative turns, counters) – only stored when something changed.
    points: std::collections::VecDeque<(u64, u64, NetCounters)>,
    keep_ns: u64,
}

impl NetHistory {
    pub fn new(keep_secs: u64) -> Self {
        Self { keep_ns: keep_secs * 1_000_000_000, ..Default::default() }
    }

    pub fn push(&mut self, t_ns: u64, c: NetCounters) {
        self.counter.feed(c);
        let changed = self.points.back().map(|p| p.2 != c).unwrap_or(true);
        if changed {
            self.points.push_back((t_ns, self.counter.turns, c));
        }
        while let Some(&(t0, ..)) = self.points.front() {
            if t_ns.saturating_sub(t0) > self.keep_ns && self.points.len() > 1 {
                self.points.pop_front();
            } else {
                break;
            }
        }
    }

    fn at(&self, t_ns: u64) -> Option<&(u64, u64, NetCounters)> {
        // Last point at or before t.
        let idx = self.points.partition_point(|p| p.0 <= t_ns);
        if idx == 0 {
            self.points.front()
        } else {
            self.points.get(idx - 1)
        }
    }

    /// Activity between two monotonic times.
    pub fn window(&self, start_ns: u64, end_ns: u64) -> NetWindow {
        let (Some(a), Some(b)) = (self.at(start_ns), self.at(end_ns)) else {
            return NetWindow::default();
        };
        let (ca, cb) = (a.2, b.2);
        let same_conns = ca.connections == cb.connections;
        let d = |x: u64, y: u64| if same_conns { y.saturating_sub(x) } else { y };
        NetWindow {
            round_trips: b.1.saturating_sub(a.1),
            tx_bytes: d(ca.tx_bytes, cb.tx_bytes),
            rx_bytes: d(ca.rx_bytes, cb.rx_bytes),
            tx_packets: d(ca.tx_packets, cb.tx_packets),
            rx_packets: d(ca.rx_packets, cb.rx_packets),
            srtt_ms: cb.srtt_ms,
            connected: cb.connections > 0,
        }
    }
}

/// A probe that finds nothing. Used when the probe is disabled.
pub struct NoProbe;
impl NetProbe for NoProbe {
    fn sample(&mut self) -> Option<NetCounters> {
        None
    }
}

#[cfg(target_os = "macos")]
pub use mac::TcpPortProbe;

#[cfg(target_os = "macos")]
mod mac {
    use super::{NetCounters, NetProbe};
    use std::time::{Duration, Instant};

    /// struct tcp_connection_info from <netinet/tcp.h> (macOS SDK).
    /// Layout checked against clang: size 112, tcpi_txpackets at 56,
    /// tcpi_txbytes 64, tcpi_rxpackets 80, tcpi_rxbytes 88, tcpi_srtt 44.
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct TcpConnectionInfo {
        state: u8,
        snd_wscale: u8,
        rcv_wscale: u8,
        _pad1: u8,
        options: u32,
        flags: u32,
        rto: u32,
        maxseg: u32,
        snd_ssthresh: u32,
        snd_cwnd: u32,
        snd_wnd: u32,
        snd_sbbytes: u32,
        rcv_wnd: u32,
        rttcur: u32,
        srtt: u32,
        rttvar: u32,
        tfo_bits: u32,
        txpackets: u64,
        txbytes: u64,
        txretransmitbytes: u64,
        rxpackets: u64,
        rxbytes: u64,
        rxoutoforderbytes: u64,
        txretransmitpackets: u64,
    }
    const _: () = assert!(std::mem::size_of::<TcpConnectionInfo>() == 112);

    const TCP_CONNECTION_INFO: libc::c_int = 0x106;
    const PROC_PIDLISTFDS: libc::c_int = 1;
    const PROX_FDTYPE_SOCKET: u32 = 2;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct ProcFdInfo {
        proc_fd: i32,
        proc_fdtype: u32,
    }

    extern "C" {
        fn proc_pidinfo(pid: libc::c_int, flavor: libc::c_int, arg: u64, buffer: *mut libc::c_void, size: libc::c_int) -> libc::c_int;
    }

    /// Sockets of *this* process whose remote port is `port`.
    pub struct TcpPortProbe {
        port: u16,
        fds: Vec<i32>,
        last_scan: Option<Instant>,
        rescan: Duration,
    }

    impl TcpPortProbe {
        pub fn new(port: u16) -> Self {
            Self { port, fds: Vec::new(), last_scan: None, rescan: Duration::from_secs(2) }
        }

        pub fn port(&self) -> u16 {
            self.port
        }

        /// List socket fds with libproc, keep TCP ones whose peer port matches.
        fn scan(&mut self) {
            self.fds.clear();
            let pid = unsafe { libc::getpid() };
            let mut buf = vec![ProcFdInfo::default(); 1024];
            let bytes = unsafe {
                proc_pidinfo(pid, PROC_PIDLISTFDS, 0, buf.as_mut_ptr().cast(), (buf.len() * 8) as libc::c_int)
            };
            self.last_scan = Some(Instant::now());
            if bytes <= 0 {
                return;
            }
            let n = bytes as usize / std::mem::size_of::<ProcFdInfo>();
            for fi in &buf[..n.min(buf.len())] {
                if fi.proc_fdtype == PROX_FDTYPE_SOCKET && peer_port(fi.proc_fd) == Some(self.port) {
                    self.fds.push(fi.proc_fd);
                }
            }
        }
    }

    fn peer_port(fd: i32) -> Option<u16> {
        unsafe {
            let mut ty: libc::c_int = 0;
            let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            if libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE, (&mut ty as *mut libc::c_int).cast(), &mut len) != 0
                || ty != libc::SOCK_STREAM
            {
                return None;
            }
            let mut ss: libc::sockaddr_storage = std::mem::zeroed();
            let mut sl = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            if libc::getpeername(fd, (&mut ss as *mut libc::sockaddr_storage).cast(), &mut sl) != 0 {
                return None;
            }
            match ss.ss_family as libc::c_int {
                libc::AF_INET => {
                    let a = &*(&ss as *const _ as *const libc::sockaddr_in);
                    Some(u16::from_be(a.sin_port))
                }
                libc::AF_INET6 => {
                    let a = &*(&ss as *const _ as *const libc::sockaddr_in6);
                    Some(u16::from_be(a.sin6_port))
                }
                _ => None,
            }
        }
    }

    fn conn_info(fd: i32) -> Option<TcpConnectionInfo> {
        let mut info = TcpConnectionInfo::default();
        let mut len = std::mem::size_of::<TcpConnectionInfo>() as libc::socklen_t;
        let r = unsafe {
            libc::getsockopt(fd, libc::IPPROTO_TCP, TCP_CONNECTION_INFO, (&mut info as *mut TcpConnectionInfo).cast(), &mut len)
        };
        (r == 0).then_some(info)
    }

    impl NetProbe for TcpPortProbe {
        fn sample(&mut self) -> Option<NetCounters> {
            // Re-scan every 2 s, or every 200 ms while no connection is known
            // (the user may open a hosted file after the recorder starts).
            let every = if self.fds.is_empty() { Duration::from_millis(200) } else { self.rescan };
            let stale = self.last_scan.map(|t| t.elapsed() >= every).unwrap_or(true);
            if stale {
                self.scan();
            }
            let mut c = NetCounters::default();
            let mut busiest = 0u64;
            let mut lost = false;
            for &fd in &self.fds {
                // An fd number can be closed and reused; the next scan fixes it.
                match conn_info(fd) {
                    Some(i) => {
                        c.tx_bytes += i.txbytes;
                        c.rx_bytes += i.rxbytes;
                        c.tx_packets += i.txpackets;
                        c.rx_packets += i.rxpackets;
                        c.connections += 1;
                        if i.txbytes + i.rxbytes >= busiest {
                            busiest = i.txbytes + i.rxbytes;
                            c.srtt_ms = i.srtt;
                        }
                    }
                    None => lost = true,
                }
            }
            if lost {
                self.last_scan = None;
            }
            Some(c)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(tx: u64, rx: u64) -> NetCounters {
        NetCounters { tx_bytes: tx, rx_bytes: rx, connections: 1, ..Default::default() }
    }

    #[test]
    fn counts_request_then_reply() {
        let mut t = TurnCounter::default();
        assert_eq!(t.feed(c(0, 0)), 0);
        assert_eq!(t.feed(c(100, 0)), 0); // request out
        assert_eq!(t.feed(c(100, 0)), 0); // waiting
        assert_eq!(t.feed(c(100, 900)), 1); // reply
        assert_eq!(t.feed(c(100, 1900)), 0); // more of the same reply
        assert_eq!(t.feed(c(150, 1900)), 0);
        assert_eq!(t.feed(c(150, 2500)), 1);
        assert_eq!(t.turns, 2);
    }

    #[test]
    fn request_and_reply_in_one_tick_counts_once() {
        let mut t = TurnCounter::default();
        t.feed(c(0, 0));
        assert_eq!(t.feed(c(50, 70)), 1);
        assert_eq!(t.feed(c(100, 140)), 1);
        assert_eq!(t.turns, 2);
    }

    #[test]
    fn server_push_without_request_is_not_a_turn() {
        let mut t = TurnCounter::default();
        t.feed(c(0, 0));
        assert_eq!(t.feed(c(0, 300)), 0);
        assert_eq!(t.turns, 0);
    }

    #[test]
    fn reconnect_resets() {
        let mut t = TurnCounter::default();
        t.feed(c(1000, 1000));
        t.feed(c(1100, 1000));
        assert_eq!(t.feed(c(10, 20)), 0); // new connection, lower counters
        assert_eq!(t.feed(c(10, 40)), 0); // reply to nothing we saw
        assert_eq!(t.feed(c(30, 40)), 0);
        assert_eq!(t.feed(c(30, 90)), 1);
    }

    #[test]
    fn history_window_counts_inside_range() {
        let mut h = NetHistory::new(60);
        let ms = 1_000_000u64;
        let mut tx = 0;
        let mut rx = 0;
        h.push(0, c(0, 0));
        // 10 round trips, one every 100 ms, starting at t=100ms.
        for i in 1..=10u64 {
            tx += 40;
            h.push(i * 100 * ms, c(tx, rx));
            rx += 400;
            h.push(i * 100 * ms + 30 * ms, c(tx, rx));
        }
        let w = h.window(0, 2_000 * ms);
        assert_eq!(w.round_trips, 10);
        assert_eq!(w.tx_bytes, 400);
        assert_eq!(w.rx_bytes, 4000);
        assert!(w.connected);
        // Only trips 3..=5 finished between 250 ms and 560 ms.
        let w = h.window(250 * ms, 560 * ms);
        assert_eq!(w.round_trips, 3);
    }

    #[test]
    fn history_trims_old_points() {
        let mut h = NetHistory::new(1);
        for i in 0..100u64 {
            h.push(i * 100_000_000, c(i, i));
        }
        assert!(h.points.len() <= 12, "{}", h.points.len());
    }

    #[test]
    fn empty_history_is_not_connected() {
        let h = NetHistory::new(60);
        assert_eq!(h.window(0, 10), NetWindow::default());
    }
}
