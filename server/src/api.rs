//! The JSON API: ingest (docs/ingest-api.md), health, token, status.

use crate::app::{with_db, Shared, VERSION};
use crate::store::{self, Meta, StoreError};
use crate::tz::now_ms;
use crate::views::{self, Ctx, Params};
use axum::body::{to_bytes, Body};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use fm_flight_recorder::wire::{ErrorReply, MAX_BATCH_BYTES, PATH_EVENTS, PATH_HEALTH, PATH_PING};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route(PATH_HEALTH, get(health))
        .route(PATH_EVENTS, post(events))
        .route(PATH_PING, post(ping))
        .route("/api/v1/token", get(token_show).post(token_new))
        .route("/api/v1/status", get(status))
        .route("/api/v1/flags", get(flags_json))
        .route("/api/v1/freezes", get(freezes_json))
        // The ingest routes read the body themselves, with the API limit.
        .layer(axum::extract::DefaultBodyLimit::disable())
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ErrorReply { error: msg.into() })).into_response()
}

fn store_err(e: StoreError) -> Response {
    match e {
        StoreError::Bad(m) => err(StatusCode::BAD_REQUEST, m),
        StoreError::TooLarge => err(StatusCode::PAYLOAD_TOO_LARGE, "batch too large"),
        StoreError::Db(m) => {
            crate::logln!("database error: {m}");
            err(StatusCode::SERVICE_UNAVAILABLE, "database error")
        }
    }
}

/// Compare in constant time, so the reply time does not tell the token.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn authorized(app: &Shared, h: &HeaderMap) -> bool {
    let token = app.token();
    h.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| !token.is_empty() && same(t.trim().as_bytes(), token.as_bytes()))
}

/// Read the body with the API byte limit: over it is 413.
async fn body_bytes(h: &HeaderMap, body: Body) -> Result<axum::body::Bytes, Response> {
    let too_big = || err(StatusCode::PAYLOAD_TOO_LARGE, "batch too large");
    if let Some(len) = h
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
    {
        if len > MAX_BATCH_BYTES {
            return Err(too_big());
        }
    }
    to_bytes(body, MAX_BATCH_BYTES).await.map_err(|_| too_big())
}

/// A browser POST from another site must not change the server. The
/// dashboard has no sign-in, so the dashboard and token calls refuse a
/// request whose Origin (or Referer) is not this server. A request with
/// neither header (curl, scripts) passes.
pub fn same_origin(h: &HeaderMap) -> bool {
    let host = h
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let from = h
        .get(header::ORIGIN)
        .or_else(|| h.get(header::REFERER))
        .and_then(|v| v.to_str().ok());
    match from {
        None => true,
        Some(u) => {
            let rest = u.split_once("://").map(|x| x.1).unwrap_or("");
            let authority = rest.split('/').next().unwrap_or("");
            !host.is_empty() && authority.eq_ignore_ascii_case(host)
        }
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"ok": true, "version": VERSION}))
}

async fn events(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    h: HeaderMap,
    body: Body,
) -> Response {
    if !authorized(&app, &h) {
        return err(StatusCode::UNAUTHORIZED, "bad token");
    }
    let bytes = match body_bytes(&h, body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let batch = match store::parse_batch(&bytes) {
        Ok(b) => b,
        Err(e) => return store_err(e),
    };
    let ip = peer.ip().to_string();
    let r = with_db(&app, move |a, db| {
        let now = now_ms();
        let s = a.settings(db);
        let meta = Meta {
            ip: &ip,
            seen_ms: now,
            zone: a.zone,
            cutoff_day: a.cutoff_day(&s, now),
        };
        store::store_batch(db, &batch, meta)
    })
    .await;
    match r {
        Ok(reply) => Json(reply).into_response(),
        Err(e) => store_err(e),
    }
}

async fn ping(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    h: HeaderMap,
    body: Body,
) -> Response {
    if !authorized(&app, &h) {
        return err(StatusCode::UNAUTHORIZED, "bad token");
    }
    let bytes = match body_bytes(&h, body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let p = match store::parse_ping(&bytes) {
        Ok(p) => p,
        Err(e) => return store_err(e),
    };
    let ip = peer.ip().to_string();
    match with_db(&app, move |_, db| store::store_ping(db, &p, &ip, now_ms())).await {
        Ok(()) => Json(json!({})).into_response(),
        Err(e) => store_err(e),
    }
}

/// The Event token for the Setup page. The dashboard has no sign-in on the
/// LAN (a decision of the design), so this call has none either.
async fn token_show(State(app): State<Shared>) -> Json<serde_json::Value> {
    Json(json!({"token": app.token()}))
}

async fn token_new(State(app): State<Shared>, h: HeaderMap) -> Response {
    if !same_origin(&h) {
        return err(StatusCode::FORBIDDEN, "cross-site request");
    }
    let a = app.clone();
    match tokio::task::spawn_blocking(move || a.new_token()).await {
        Ok(Ok(t)) => Json(json!({"token": t})).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "cannot make a token"),
    }
}

async fn status(State(app): State<Shared>) -> Json<serde_json::Value> {
    let f = with_db(&app, |a, db| crate::query::footer(a, db, now_ms())).await;
    Json(serde_json::to_value(f).unwrap())
}

/// `GET /api/v1/flags?range=&file=`: the flagged layouts, clients, versions.
async fn flags_json(
    State(app): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    with_db(&app, move |a, db| {
        let ctx = Ctx::new(a, db, now_ms());
        let p = Params::from_map(&q);
        let r = p.range(ctx.z, ctx.now);
        Json(ctx.flags(&r, &p.file_opt())).into_response()
    })
    .await
}

/// `GET /api/v1/freezes?range=&file=&layout=&account=&script=&client=&before=`
async fn freezes_json(
    State(app): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    with_db(&app, move |a, db| {
        let ctx = Ctx::new(a, db, now_ms());
        let p = Params::from_map(&q);
        let v = views::freezes(&ctx, &p);
        Json(serde_json::json!({
            "total": v.total,
            "rows": v.rows,
            "next": v.more_url.replace("/freezes/more", "/api/v1/freezes"),
        }))
        .into_response()
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn h(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(k.clone(), HeaderValue::from_str(v).unwrap());
        }
        m
    }

    #[test]
    fn cross_site_posts_are_refused() {
        assert!(same_origin(&h(&[(header::HOST, "192.168.1.10:8787")])));
        assert!(same_origin(&h(&[
            (header::HOST, "192.168.1.10:8787"),
            (header::ORIGIN, "http://192.168.1.10:8787")
        ])));
        assert!(same_origin(&h(&[
            (header::HOST, "a:1"),
            (header::REFERER, "http://a:1/setup?x=1")
        ])));
        assert!(!same_origin(&h(&[
            (header::HOST, "192.168.1.10:8787"),
            (header::ORIGIN, "http://evil.example")
        ])));
        assert!(!same_origin(&h(&[
            (header::HOST, "a:1"),
            (header::ORIGIN, "null")
        ])));
        assert!(!same_origin(&h(&[
            (header::HOST, "a:1"),
            (header::REFERER, "http://b:1/")
        ])));
    }
}
