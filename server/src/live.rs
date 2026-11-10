//! Live updates for open dashboard pages: one Server-Sent Events stream,
//! `GET /api/v1/live`.
//!
//! The stream carries no event data, only a hint that the page is out of
//! date:
//!
//! * `hello`: sent once when the stream opens (data: the server version).
//! * `events`: the server stored new events (data: how many).
//! * `client`: a client connected or went offline (data: how many).
//!
//! The page then fetches its own URL again and swaps the parts that changed
//! (`static/app.js`), so every page keeps its filters and works without JS.
//! A comment line goes out every 15 s so proxies and browsers keep the
//! connection open. On shutdown all streams end, so the graceful shutdown
//! of the server does not wait for open pages.

use crate::app::{Shared, VERSION};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering::*};
use std::time::Duration;
use tokio::sync::{broadcast, watch};

/// Open streams at most (each open page has one). More get `503`.
pub const MAX_STREAMS: usize = 64;
/// The comment interval that keeps an idle stream open.
pub const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// How long the browser waits before it opens a dropped stream again.
pub const RETRY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Events,
    Client,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Events => "events",
            Kind::Client => "client",
        }
    }
}

pub struct Live {
    tx: broadcast::Sender<(Kind, u64)>,
    stop: watch::Sender<bool>,
    streams: AtomicUsize,
}

impl Default for Live {
    fn default() -> Self {
        Self::new()
    }
}

impl Live {
    pub fn new() -> Live {
        let (tx, _) = broadcast::channel(64);
        let (stop, _) = watch::channel(false);
        Live {
            tx,
            stop,
            streams: AtomicUsize::new(0),
        }
    }

    /// Tell all open pages. Never blocks; without a page it does nothing.
    pub fn notify(&self, kind: Kind, n: u64) {
        if n > 0 {
            let _ = self.tx.send((kind, n));
        }
    }

    /// End all streams (shutdown). New streams end at once.
    pub fn close(&self) {
        let _ = self.stop.send(true);
    }

    /// The number of open streams.
    pub fn streams(&self) -> usize {
        self.streams.load(Relaxed)
    }
}

/// Counts one open stream; the count goes down when the stream is dropped
/// (the page closed, or the server stops).
struct Slot(Shared);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.live.streams.fetch_sub(1, Relaxed);
    }
}

struct St {
    rx: broadcast::Receiver<(Kind, u64)>,
    stop: watch::Receiver<bool>,
    hello: bool,
    _slot: Slot,
}

/// `GET /api/v1/live`. No token: the stream has no data, like the pages.
pub async fn stream(State(app): State<Shared>) -> Response {
    let live = &app.live;
    if live.streams.fetch_add(1, Relaxed) >= MAX_STREAMS {
        live.streams.fetch_sub(1, Relaxed);
        return (StatusCode::SERVICE_UNAVAILABLE, "too many live pages").into_response();
    }
    let st = St {
        rx: live.tx.subscribe(),
        stop: live.stop.subscribe(),
        hello: true,
        _slot: Slot(app.clone()),
    };
    let s = futures_util::stream::unfold(st, |mut st| async move {
        if st.hello {
            st.hello = false;
            let ev = Event::default().event("hello").data(VERSION).retry(RETRY);
            return Some((Ok::<_, Infallible>(ev), st));
        }
        if *st.stop.borrow() {
            return None;
        }
        let ev = tokio::select! {
            r = st.rx.recv() => match r {
                Ok((k, n)) => Event::default().event(k.name()).data(n.to_string()),
                // Missed some: the page reloads its data anyway.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    Event::default().event(Kind::Events.name()).data("0")
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            },
            _ = st.stop.wait_for(|v| *v) => return None,
        };
        Some((Ok(ev), st))
    });
    Sse::new(s)
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
        .into_response()
}
