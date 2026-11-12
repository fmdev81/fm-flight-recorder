//! The running server: shared state, the router, the background jobs.

use crate::config::Config;
use crate::db::{self, Db, Settings};
use crate::live::{Kind, Live};
use crate::tz::{now_ms, Zone};
use crate::{api, logln, rollup, web};
use axum::Router;
use chrono::NaiveDate;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct App {
    pub cfg: Config,
    pub zone: Zone,
    pub db: Mutex<Db>,
    /// The Event token, cached; the database keeps it.
    pub token: RwLock<String>,
    pub started_ms: i64,
    /// The address the server listens on, once it is bound.
    pub bound: OnceLock<SocketAddr>,
    /// Tells open dashboard pages about new data (`GET /api/v1/live`).
    pub live: Live,
}

pub type Shared = Arc<App>;

impl App {
    pub fn new(cfg: Config) -> Result<App, String> {
        let zone = Zone::new(&cfg.tz)?;
        let db = db::open(&cfg.db)?;
        let token = db::ensure_token(&db).map_err(|e| e.to_string())?;
        Ok(App {
            cfg,
            zone,
            db: Mutex::new(db),
            token: RwLock::new(token),
            started_ms: now_ms(),
            bound: OnceLock::new(),
            live: Live::new(),
        })
    }

    pub fn token(&self) -> String {
        self.token.read().unwrap().clone()
    }

    /// Make a new Event token. The old one gets 401 from now on.
    pub fn new_token(&self) -> Result<String, String> {
        let db = self.db.lock().unwrap();
        let t = db::replace_token(&db).map_err(|e| e.to_string())?;
        *self.token.write().unwrap() = t.clone();
        Ok(t)
    }

    pub fn settings(&self, db: &Db) -> Settings {
        Settings::load(db, &self.cfg.defaults)
    }

    /// The first day that still has its raw events (older days use the
    /// daily totals).
    pub fn cutoff_day(&self, s: &Settings, now: i64) -> NaiveDate {
        rollup::cutoff_day(self.zone, s.event_days, now)
    }

    /// The listen address (configured, or bound).
    pub fn listen_text(&self) -> String {
        self.bound
            .get()
            .map(|a| a.to_string())
            .unwrap_or_else(|| self.cfg.listen.clone())
    }

    /// The URL that the plug-in uses (`endpoint=`).
    pub fn public_url(&self) -> String {
        if !self.cfg.public_url.is_empty() {
            return self.cfg.public_url.trim_end_matches('/').to_string();
        }
        let addr: Option<SocketAddr> = self
            .bound
            .get()
            .copied()
            .or_else(|| self.cfg.listen.parse().ok());
        let port = addr.map(|a| a.port()).unwrap_or(8787);
        let ip = match addr.map(|a| a.ip()) {
            Some(ip) if !ip.is_unspecified() => ip,
            _ => lan_ip().unwrap_or(IpAddr::from([127, 0, 0, 1])),
        };
        match ip {
            IpAddr::V6(v6) => format!("http://[{v6}]:{port}"),
            IpAddr::V4(v4) => format!("http://{v4}:{port}"),
        }
    }

    pub fn connected_ms(&self) -> i64 {
        self.cfg.connected_secs as i64 * 1000
    }
}

/// The LAN address of this machine: the source address of a route to a
/// private address. No packet is sent.
pub fn lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.255.255:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// Run a database call on the blocking pool.
pub async fn with_db<T, F>(app: &Shared, f: F) -> T
where
    T: Send + 'static,
    F: FnOnce(&App, &mut Db) -> T + Send + 'static,
{
    let a = app.clone();
    tokio::task::spawn_blocking(move || {
        let mut g = a.db.lock().unwrap_or_else(|p| p.into_inner());
        f(&a, &mut g)
    })
    .await
    .expect("database task")
}

pub fn router(app: Shared) -> Router {
    Router::new()
        .merge(api::routes())
        .merge(web::routes())
        .with_state(app)
}

/// The jobs: close the sessions of offline clients (each minute), daily
/// totals and retention (at start, then each hour).
pub fn spawn_jobs(app: Shared) {
    tokio::spawn(async move {
        let mut tick = 0u64;
        loop {
            let a = app.clone();
            let hourly = tick.is_multiple_of(60);
            let r = tokio::task::spawn_blocking(move || {
                let mut db = a.db.lock().unwrap_or_else(|p| p.into_inner());
                let now = now_ms();
                let s = a.settings(&db);
                // `fr-server token --new` changes the token in the database.
                if let Some(t) = db::get_setting(&db, db::TOKEN_KEY).filter(|t| !t.is_empty()) {
                    *a.token.write().unwrap() = t;
                }
                let closed = rollup::close_offline_sessions(&db, now, a.connected_ms())?;
                // A client went offline: the Clients page and the footer change.
                a.live.notify(Kind::Client, closed as u64);
                let ended = rollup::close_open_freezes(&db, now)?;
                // An open freeze ended: the freeze lists change.
                a.live.notify(Kind::Events, ended as u64);
                let mut out = format!("closed {closed} sessions");
                if hourly {
                    let rep = rollup::run(&mut db, a.zone, &s, now)?;
                    out = format!("{out}; {rep}");
                }
                Ok::<_, rusqlite::Error>((hourly, out))
            })
            .await;
            match r {
                Ok(Ok((true, msg))) => logln!("jobs: {msg}"),
                Ok(Ok(_)) => {}
                Ok(Err(e)) => logln!("jobs failed: {e}"),
                Err(e) => logln!("jobs failed: {e}"),
            }
            tick += 1;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

/// Bind, start the jobs and serve until Ctrl-C (or `stop` resolves).
pub async fn serve(
    app: Shared,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(&app.cfg.listen)
        .await
        .map_err(|e| format!("cannot listen on {}: {e}", app.cfg.listen))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let _ = app.bound.set(addr);
    logln!(
        "fr-server {VERSION} listening on {addr}; dashboard {}; database {}; time zone {}",
        app.public_url(),
        app.cfg.db.display(),
        app.zone.name()
    );
    spawn_jobs(app.clone());
    // End the live streams first, or the graceful shutdown waits for every
    // open page.
    let a = app.clone();
    let stop = async move {
        stop.await;
        a.live.close();
    };
    axum::serve(
        listener,
        router(app).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(stop)
    .await
    .map_err(|e| e.to_string())
}

/// Start a server on a free local port in a background thread (tests).
/// Returns the state and the base URL.
pub fn spawn_for_test(cfg: Config) -> (Shared, String) {
    let app = Arc::new(App::new(cfg).expect("app"));
    let a = app.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let _ = a.bound.set(addr);
            tx.send(addr).unwrap();
            axum::serve(
                listener,
                router(a).into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
    });
    let addr = rx.recv().unwrap();
    (app, format!("http://{addr}"))
}
