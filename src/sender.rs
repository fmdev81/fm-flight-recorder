//! The `fr-sender` thread: posts event batches to the companion server
//! (docs/ingest-api.md).
//!
//! Data flow: the writer thread writes each line to the JSONL log first,
//! then pushes the same text into the [`SendQueue`]. The sender takes
//! batches from the queue and posts them.
//!
//! Rules:
//! * FileMaker's main thread never touches the queue or the network.
//! * The queue is bounded. When it is full, the oldest unsent event is
//!   dropped and counted. The JSONL log still has it.
//! * The queue lock is held only to add, peek or remove items, never
//!   during I/O. A batch stays in the queue until the server accepts it,
//!   or until it is in the spool.
//! * When a post fails, the batch goes to the disk spool ([`Spool`]), and
//!   the next try waits longer each time (1 s, 2 s, 4 s … 5 min). The spool
//!   is always sent before newer events, also after a restart.
//! * [`Sender::finish`] never waits for the network: it closes the socket
//!   in use, waits a few ms for the thread, and detaches it if it is still
//!   in `connect`. The writer thread puts the unsent events in the spool.

use crate::http::{self, Abort, Timeouts};
use crate::recorder::Endpoint;
use crate::spool::Spool;
use crate::wire::{ClientInfo, IngestReply, API_VERSION, PATH_EVENTS};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering::*};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct SendSettings {
    pub endpoint: Endpoint,
    pub token: String,
    /// A batch goes out at most this long after its first event...
    pub interval: Duration,
    /// ...or as soon as this many events wait.
    pub batch_max: usize,
    /// Byte limit of one batch body (about; one line can pass it alone).
    pub batch_bytes: usize,
    /// Events kept in memory for sending.
    pub queue_max: usize,
    pub timeouts: Timeouts,
    /// Wait after the first failed post; it doubles after each failure...
    pub retry: Duration,
    /// ...up to this.
    pub retry_max: Duration,
    /// Folder for unsent batches. None = no spool (keep them in memory).
    pub spool_dir: Option<PathBuf>,
    /// Size cap of the spool; 0 = no spool.
    pub spool_max_bytes: u64,
}

/// Counters, read by Status. Written by the writer and sender threads.
#[derive(Debug, Default)]
pub struct SendStats {
    /// Events the server accepted (or had already).
    pub sent: AtomicU64,
    /// Events dropped: queue full, spool full, refused by the server
    /// (400/413), or not sent at stop without a spool.
    pub dropped: AtomicU64,
    /// Events in the spool now.
    pub spooled: AtomicU64,
    /// Events the server said it had already (resends).
    pub duplicates: AtomicU64,
    /// Failed posts (network error, timeout, 401, 5xx).
    pub failures: AtomicU64,
    /// Unix ms of the last accepted batch, 0 = never.
    pub last_send_unix_ms: AtomicI64,
    /// The server answered 401: sending stops until the next start.
    pub auth_failed: AtomicBool,
    pub last_error: Mutex<String>,
}

impl SendStats {
    pub fn set_error(&self, e: impl Into<String>) {
        *self.last_error.lock().unwrap() = e.into();
    }
}

struct Item {
    seq: u64,
    line: String,
    at: Instant,
}

#[derive(Default)]
struct QState {
    items: VecDeque<Item>,
    /// The sender must exit.
    stop: bool,
    /// No more pushes; the writer took the rest.
    closed: bool,
}

/// Bounded queue of JSONL lines waiting to be sent.
pub struct SendQueue {
    st: Mutex<QState>,
    cv: Condvar,
    max: usize,
}

impl SendQueue {
    pub fn new(max: usize) -> Self {
        Self {
            st: Mutex::new(QState::default()),
            cv: Condvar::new(),
            max: max.max(1),
        }
    }

    /// Add one line. Returns how many old events were dropped to make room.
    pub fn push(&self, seq: u64, line: String) -> u64 {
        let mut st = self.st.lock().unwrap();
        if st.closed {
            return 1;
        }
        let mut dropped = 0;
        while st.items.len() >= self.max {
            st.items.pop_front();
            dropped += 1;
        }
        st.items.push_back(Item {
            seq,
            line,
            at: Instant::now(),
        });
        drop(st);
        self.cv.notify_one();
        dropped
    }

    pub fn len(&self) -> usize {
        self.st.lock().unwrap().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn is_stopped(&self) -> bool {
        let st = self.st.lock().unwrap();
        st.stop || st.closed
    }

    fn stop(&self) {
        self.st.lock().unwrap().stop = true;
        self.cv.notify_all();
    }

    /// No more pushes. Returns the events that were not sent, in order.
    pub fn close(&self) -> Vec<(u64, String)> {
        let mut st = self.st.lock().unwrap();
        st.closed = true;
        st.stop = true;
        let out = st.items.drain(..).map(|i| (i.seq, i.line)).collect();
        drop(st);
        self.cv.notify_all();
        out
    }

    /// Remove the events up to `last_seq` (they were sent or refused).
    /// Returns how many were removed.
    fn commit(&self, last_seq: u64) -> u64 {
        let mut st = self.st.lock().unwrap();
        let mut n = 0;
        while st.items.front().is_some_and(|i| i.seq <= last_seq) {
            st.items.pop_front();
            n += 1;
        }
        n
    }
}

/// The JSON body of `POST /api/v1/events` for these lines.
pub fn batch_body(session_id: &str, client: &ClientInfo, lines: &[(u64, String)]) -> String {
    let mut b = String::with_capacity(lines.iter().map(|x| x.1.len() + 1).sum::<usize>() + 512);
    b.push_str(&format!(
        "{{\"v\":{API_VERSION},\"session_id\":{},\"client\":{},\"events\":[",
        serde_json::to_string(session_id).unwrap(),
        serde_json::to_string(client).unwrap()
    ));
    for (i, (_, line)) in lines.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push_str(line);
    }
    b.push_str("]}");
    b
}

/// The running sender thread.
pub struct Sender {
    pub queue: Arc<SendQueue>,
    pub stats: Arc<SendStats>,
    spool: Option<Arc<Spool>>,
    session_id: String,
    client: Arc<OnceLock<ClientInfo>>,
    batch_max: usize,
    abort: Abort,
    thread: Mutex<Option<JoinHandle<()>>>,
    done: Mutex<Receiver<()>>,
}

impl Sender {
    pub fn start(
        settings: SendSettings,
        session_id: String,
        client: Arc<OnceLock<ClientInfo>>,
    ) -> Sender {
        let queue = Arc::new(SendQueue::new(settings.queue_max));
        let stats = Arc::new(SendStats::default());
        let abort = Abort::default();
        let spool = settings
            .spool_dir
            .clone()
            .filter(|_| settings.spool_max_bytes > 0)
            .map(|d| Arc::new(Spool::new(d, settings.spool_max_bytes)));
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let batch_max = settings.batch_max;
        let worker = Worker {
            s: settings,
            queue: queue.clone(),
            stats: stats.clone(),
            spool: spool.clone(),
            session_id: session_id.clone(),
            client: client.clone(),
            abort: abort.clone(),
        };
        let thread = thread::Builder::new()
            .name("fr-sender".into())
            .spawn(move || {
                // Dropped when the thread ends, also on a panic.
                let _done = done_tx;
                worker.run();
            })
            .expect("spawn sender");
        Sender {
            queue,
            stats,
            spool,
            session_id,
            client,
            batch_max,
            abort,
            thread: Mutex::new(Some(thread)),
            done: Mutex::new(done_rx),
        }
    }

    /// Tell the thread to stop and cut its network call. Does not wait.
    pub fn signal_stop(&self) {
        self.queue.stop();
        self.abort.abort();
    }

    /// Wait at most `max` for the thread to end. If it does not end (it can
    /// be in `connect` to a host that does not answer), detach it: it ends
    /// on its own when its connect timeout expires. Returns true if it ended.
    pub fn finish(&self, max: Duration) -> bool {
        self.signal_stop();
        let Some(t) = self.thread.lock().unwrap().take() else {
            return true;
        };
        let ended = matches!(
            self.done.lock().unwrap().recv_timeout(max),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        if ended {
            let _ = t.join();
        }
        ended
    }

    /// At stop (writer thread): put the events that were not sent into the
    /// spool, or count them as dropped if there is no spool.
    pub fn spool_rest(&self, items: Vec<(u64, String)>) {
        if items.is_empty() {
            return;
        }
        let st = &self.stats;
        let Some(spool) = &self.spool else {
            st.dropped.fetch_add(items.len() as u64, Relaxed);
            st.set_error(format!("{} events not sent at stop", items.len()));
            return;
        };
        let client = self.client.get().cloned().unwrap_or_default();
        for chunk in items.chunks(self.batch_max.max(1)) {
            let body = batch_body(&self.session_id, &client, chunk);
            let n = chunk.len() as u64;
            match spool.write(&body, &self.session_id, chunk[0].0, n) {
                Ok(dropped) => {
                    st.dropped.fetch_add(dropped, Relaxed);
                }
                Err(e) => {
                    st.dropped.fetch_add(n, Relaxed);
                    st.set_error(format!("spool {}: {e}", spool.dir().display()));
                }
            }
        }
        st.spooled.store(spool.count().events, Relaxed);
    }
}

struct Worker {
    s: SendSettings,
    queue: Arc<SendQueue>,
    stats: Arc<SendStats>,
    spool: Option<Arc<Spool>>,
    session_id: String,
    client: Arc<OnceLock<ClientInfo>>,
    abort: Abort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Post {
    /// 200: the server has the events.
    Accepted,
    /// 400/413: the server will never take this batch. Dropped.
    Refused,
    /// 401: bad token. Sending stops until the next start.
    Unauthorized,
    /// Network error, timeout, other status: try again later.
    Failed,
}

enum Next {
    Stop,
    /// `wake` time reached, no batch due.
    Wake,
    Batch(Vec<(u64, String)>),
}

/// Retry timing: 1 s, 2 s, 4 s, … up to the maximum.
struct Backoff {
    first: Duration,
    max: Duration,
    cur: Option<Duration>,
    until: Option<Instant>,
}

impl Backoff {
    fn failed(&mut self) {
        let d = match self.cur {
            None => self.first,
            Some(d) => (d * 2).min(self.max),
        };
        self.cur = Some(d);
        self.until = Some(Instant::now() + d);
    }
    fn hold(&mut self, d: Duration) {
        self.until = Some(Instant::now() + d);
    }
    fn ok(&mut self) {
        self.cur = None;
        self.until = None;
    }
    fn ready(&self) -> bool {
        self.until.is_none_or(|t| Instant::now() >= t)
    }
}

impl Worker {
    fn run(self) {
        let mut b = Backoff {
            first: self.s.retry,
            max: self.s.retry_max.max(self.s.retry),
            cur: None,
            until: None,
        };
        // Spool from earlier runs is sent first.
        let mut spool_pending = self.spool.as_ref().is_some_and(|s| !s.is_empty());
        let mut spool_ok = true;
        self.update_spool_count();
        loop {
            if self.queue.is_stopped() {
                return;
            }
            let auth_failed = self.stats.auth_failed.load(Relaxed);
            // 1. The spool first, oldest file first.
            if spool_pending && !auth_failed && b.ready() {
                let spool = self.spool.as_ref().unwrap();
                match spool.oldest() {
                    None => spool_pending = false,
                    Some((path, body, n)) => match self.post(&body, n) {
                        Post::Accepted | Post::Refused => {
                            spool.remove(&path);
                            b.ok();
                        }
                        Post::Unauthorized => b.hold(Duration::from_secs(3600)),
                        Post::Failed => b.failed(),
                    },
                }
                self.update_spool_count();
                continue;
            }
            // 2. Wait for a batch from memory. While offline with a working
            // spool, due batches go to the spool at once (memory stays
            // small, and they survive a restart). Without a spool, they
            // wait in the queue until the next try.
            let spool_usable = self.spool.is_some() && spool_ok;
            let gate = if spool_usable { None } else { b.until };
            let wake = if spool_pending && !auth_failed {
                b.until
            } else {
                None
            };
            let batch = match self.next_batch(gate, wake) {
                Next::Stop => return,
                Next::Wake => continue,
                Next::Batch(batch) => batch,
            };
            let (first_seq, last_seq) = (batch[0].0, batch[batch.len() - 1].0);
            let n = batch.len() as u64;
            let client = self.client.get().cloned().unwrap_or_default();
            let body = batch_body(&self.session_id, &client, &batch);
            // Keep the order: nothing from memory goes out before the spool.
            let online = !auth_failed && b.ready() && !spool_pending;
            let result = online.then(|| self.post(&body, n));
            match result {
                Some(Post::Accepted) | Some(Post::Refused) => {
                    self.queue.commit(last_seq);
                    b.ok();
                    continue;
                }
                Some(Post::Failed) => b.failed(),
                Some(Post::Unauthorized) => b.hold(Duration::from_secs(3600)),
                None => {}
            }
            // Not sent: spool it.
            match &self.spool {
                Some(spool) => match spool.write(&body, &self.session_id, first_seq, n) {
                    Ok(dropped) => {
                        self.stats.dropped.fetch_add(dropped, Relaxed);
                        self.queue.commit(last_seq);
                        spool_pending = true;
                        spool_ok = true;
                        self.update_spool_count();
                    }
                    Err(e) => {
                        // Keep it in memory (the queue is bounded).
                        spool_ok = false;
                        self.stats
                            .set_error(format!("spool {}: {e}", spool.dir().display()));
                        if b.ready() {
                            b.failed();
                        }
                    }
                },
                None => {
                    if b.ready() {
                        b.failed();
                    }
                }
            }
        }
    }

    fn update_spool_count(&self) {
        if let Some(s) = &self.spool {
            self.stats.spooled.store(s.count().events, Relaxed);
        }
    }

    /// Wait until a batch is due (not before `gate`), or until `wake`, then
    /// copy the batch out of the queue (it stays in the queue until it is
    /// committed).
    fn next_batch(&self, gate: Option<Instant>, wake: Option<Instant>) -> Next {
        let q = &self.queue;
        let mut st = q.st.lock().unwrap();
        loop {
            if st.stop || st.closed {
                return Next::Stop;
            }
            let now = Instant::now();
            let due = st.items.front().map(|first| {
                let due = if st.items.len() >= self.s.batch_max {
                    now
                } else {
                    first.at + self.s.interval
                };
                match gate {
                    Some(g) if g > due => g,
                    _ => due,
                }
            });
            if due.is_some_and(|t| t <= now) {
                break;
            }
            if wake.is_some_and(|t| t <= now) {
                return Next::Wake;
            }
            let until = match (due, wake) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            st = match until {
                Some(t) => q.cv.wait_timeout(st, t - now).unwrap().0,
                None => q.cv.wait(st).unwrap(),
            };
        }
        let mut out = Vec::new();
        let mut bytes = 0;
        for i in st.items.iter() {
            if out.len() >= self.s.batch_max
                || (!out.is_empty() && bytes + i.line.len() > self.s.batch_bytes)
            {
                break;
            }
            bytes += i.line.len() + 1;
            out.push((i.seq, i.line.clone()));
        }
        Next::Batch(out)
    }

    /// Post one batch body of `n` events and count the result.
    fn post(&self, body: &str, n: u64) -> Post {
        let st = &self.stats;
        let r = http::post_json(
            &self.s.endpoint,
            PATH_EVENTS,
            &self.s.token,
            body.as_bytes(),
            self.s.timeouts,
            &self.abort,
        );
        match r {
            Ok(resp) if resp.status == 200 => {
                st.sent.fetch_add(n, Relaxed);
                st.last_send_unix_ms
                    .store(crate::eventlog::unix_ms_now(), Relaxed);
                match serde_json::from_str::<IngestReply>(&resp.body) {
                    Ok(r) => {
                        st.duplicates.fetch_add(r.duplicates, Relaxed);
                    }
                    Err(_) => st.set_error(format!("unexpected reply: {}", short(&resp.body))),
                }
                Post::Accepted
            }
            Ok(resp) if resp.status == 400 || resp.status == 413 => {
                st.dropped.fetch_add(n, Relaxed);
                st.set_error(format!(
                    "HTTP {}: {} events dropped: {}",
                    resp.status,
                    n,
                    short(&resp.body)
                ));
                Post::Refused
            }
            Ok(resp) if resp.status == 401 => {
                st.failures.fetch_add(1, Relaxed);
                st.auth_failed.store(true, Relaxed);
                st.set_error("HTTP 401: bad token, sending stopped until FlightRecorder_Start");
                Post::Unauthorized
            }
            Ok(resp) => {
                st.failures.fetch_add(1, Relaxed);
                st.set_error(format!("HTTP {}: {}", resp.status, short(&resp.body)));
                Post::Failed
            }
            Err(e) => {
                st.failures.fetch_add(1, Relaxed);
                st.set_error(format!("send: {e}"));
                Post::Failed
            }
        }
    }
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 200 {
        s.chars().take(200).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_drops_oldest_when_full() {
        let q = SendQueue::new(3);
        let dropped: u64 = (1..=5).map(|i| q.push(i, format!("{i}"))).sum();
        assert_eq!(dropped, 2);
        assert_eq!(q.len(), 3);
        assert_eq!(q.commit(3), 1, "only seq 3 is left of 1..=3");
        let rest = q.close();
        assert_eq!(rest, vec![(4, "4".into()), (5, "5".into())]);
        assert_eq!(q.push(6, "6".into()), 1, "closed: counted as dropped");
    }
}
