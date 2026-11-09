//! Test helpers: a server on a free port, and a small HTTP client.

#![allow(dead_code)]

use fr_server::app::{spawn_for_test, Shared};
use fr_server::config::Config;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

pub fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("frs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A running server with an empty database in a temp folder.
pub fn server(name: &str) -> (Shared, String, PathBuf) {
    let dir = tmp(name);
    let (app, url) = spawn_for_test(Config::for_test(&dir, "America/Chicago"));
    (app, url, dir)
}

pub struct Reply {
    pub status: u16,
    pub body: String,
}

/// One HTTP/1.1 request; `Connection: close`.
pub fn request(
    base: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Reply {
    let authority = base.trim_start_matches("http://");
    let mut s = TcpStream::connect(authority).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    let _ = s.write_all(body);
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let (h, b) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let body = if h
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(b)
    } else {
        b.to_string()
    };
    Reply { status, body }
}

fn dechunk(mut b: &str) -> String {
    let mut out = String::new();
    loop {
        let Some((size, rest)) = b.split_once("\r\n") else {
            break;
        };
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 || rest.len() < n {
            break;
        }
        out.push_str(&rest[..n]);
        b = rest[n..].trim_start_matches("\r\n");
    }
    out
}

pub fn get(base: &str, path: &str) -> Reply {
    request(base, "GET", path, &[], b"")
}

pub fn post_json(base: &str, path: &str, token: Option<&str>, body: &str) -> Reply {
    let auth = token.map(|t| format!("Bearer {t}"));
    let mut h = vec![("Content-Type", "application/json")];
    if let Some(a) = &auth {
        h.push(("Authorization", a.as_str()));
    }
    request(base, "POST", path, &h, body.as_bytes())
}

/// The ```json blocks of docs/ingest-api.md, in order.
pub fn doc_examples() -> Vec<String> {
    let doc = include_str!("../../../docs/ingest-api.md");
    doc.split("```json\n")
        .skip(1)
        .map(|b| b.split("```").next().unwrap().to_string())
        .collect()
}

pub fn count(app: &Shared, sql: &str) -> i64 {
    let db = app.db.lock().unwrap();
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}
