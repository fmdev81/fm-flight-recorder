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
//!   during I/O. A batch stays in the queue until the server accepts it.
//! * [`Sender::stop`] never waits for the network: it closes the socket in
//!   use, waits a few ms for the thread, and detaches it if it is still in
//!   `connect`.

use crate::http::{self, Abort, Timeouts};
use crate::recorder::Endpoint;
use crate::wire::{ClientInfo, IngestReply, API_VERSION, PATH_EVENTS};
use std::collections::VecDeque;
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
    /// Wait after a failed post before the next try.
    pub retry: Duration,
}

/// Counters, read by Status. Written by the writer and sender threads.
#[derive(Debug, Default)]
pub struct SendStats {
    /// Events the server accepted (or had already).
    pub sent: AtomicU64,
    /// Events dropped: queue full, refused by the server (400/413), or
    /// not sent at stop.
    pub dropped: AtomicU64,
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

/// The running sender thread.
pub struct Sender {
    pub queue: Arc<SendQueue>,
    pub stats: Arc<SendStats>,
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
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let worker = Worker {
            s: settings,
            queue: queue.clone(),
            stats: stats.clone(),
            session_id,
            client,
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
}

struct Worker {
    s: SendSettings,
    queue: Arc<SendQueue>,
    stats: Arc<SendStats>,
    session_id: String,
    client: Arc<OnceLock<ClientInfo>>,
    abort: Abort,
}

enum Outcome {
    /// Remove the batch from the queue.
    Done,
    /// Keep the batch; try again after this time.
    RetryAt(Instant),
}

impl Worker {
    fn run(self) {
        let mut retry_at: Option<Instant> = None;
        while let Some(batch) = self.next_batch(retry_at) {
            let last_seq = batch.last().map(|b| b.0).unwrap_or(0);
            match self.post(&batch) {
                Outcome::Done => {
                    self.queue.commit(last_seq);
                    retry_at = None;
                }
                Outcome::RetryAt(t) => retry_at = Some(t),
            }
        }
    }

    /// Wait until a batch is due, then copy it out of the queue (it stays
    /// in the queue until it is committed). None = stop.
    fn next_batch(&self, retry_at: Option<Instant>) -> Option<Vec<(u64, String)>> {
        let q = &self.queue;
        let mut st = q.st.lock().unwrap();
        loop {
            if st.stop || st.closed {
                return None;
            }
            let now = Instant::now();
            let due = st.items.front().map(|first| {
                let due = if st.items.len() >= self.s.batch_max {
                    now
                } else {
                    first.at + self.s.interval
                };
                match retry_at {
                    Some(r) if r > due => r,
                    _ => due,
                }
            });
            match due {
                Some(t) if t <= now => break,
                Some(t) => st = q.cv.wait_timeout(st, t - now).unwrap().0,
                None => st = q.cv.wait(st).unwrap(),
            }
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
        Some(out)
    }

    fn body(&self, batch: &[(u64, String)]) -> String {
        let client = self.client.get().cloned().unwrap_or_default();
        let mut b = String::with_capacity(batch.iter().map(|x| x.1.len() + 1).sum::<usize>() + 512);
        b.push_str(&format!(
            "{{\"v\":{API_VERSION},\"session_id\":{},\"client\":{},\"events\":[",
            serde_json::to_string(&self.session_id).unwrap(),
            serde_json::to_string(&client).unwrap()
        ));
        for (i, (_, line)) in batch.iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            b.push_str(line);
        }
        b.push_str("]}");
        b
    }

    fn post(&self, batch: &[(u64, String)]) -> Outcome {
        let n = batch.len() as u64;
        let st = &self.stats;
        if st.auth_failed.load(Relaxed) {
            // 401 before: do not send until the next start. Keep the
            // events (the queue drops the oldest when full).
            return Outcome::RetryAt(Instant::now() + Duration::from_secs(3600));
        }
        let body = self.body(batch);
        let r = http::post_json(
            &self.s.endpoint,
            PATH_EVENTS,
            &self.s.token,
            body.as_bytes(),
            self.s.timeouts,
            &self.abort,
        );
        let retry = Outcome::RetryAt(Instant::now() + self.s.retry);
        match r {
            Ok(resp) if resp.status == 200 => {
                st.sent.fetch_add(n, Relaxed);
                st.last_send_unix_ms
                    .store(crate::eventlog::unix_ms_now(), Relaxed);
                if serde_json::from_str::<IngestReply>(&resp.body).is_err() {
                    st.set_error(format!("unexpected reply: {}", short(&resp.body)));
                }
                Outcome::Done
            }
            Ok(resp) if resp.status == 400 || resp.status == 413 => {
                st.dropped.fetch_add(n, Relaxed);
                st.set_error(format!(
                    "HTTP {}: {} events dropped: {}",
                    resp.status,
                    n,
                    short(&resp.body)
                ));
                Outcome::Done
            }
            Ok(resp) if resp.status == 401 => {
                st.failures.fetch_add(1, Relaxed);
                st.auth_failed.store(true, Relaxed);
                st.set_error("HTTP 401: bad token, sending stopped until FlightRecorder_Start");
                Outcome::RetryAt(Instant::now() + Duration::from_secs(3600))
            }
            Ok(resp) => {
                st.failures.fetch_add(1, Relaxed);
                st.set_error(format!("HTTP {}: {}", resp.status, short(&resp.body)));
                retry
            }
            Err(e) => {
                st.failures.fetch_add(1, Relaxed);
                st.set_error(format!("send: {e}"));
                retry
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
