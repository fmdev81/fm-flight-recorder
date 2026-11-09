//! A very small HTTP/1.1 client for the ingest API: one POST per
//! connection, plain HTTP, short timeouts. std only, no TLS (#7, #55).
//!
//! Used only on the `fr-sender` thread. [`Abort`] lets another thread cut
//! a call short: it shuts the socket down, so a blocked read or write
//! returns at once.

use crate::recorder::Endpoint;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    /// Each read and each write, and the whole reply.
    pub io: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// The socket of the call in progress, so that another thread can close it.
#[derive(Debug, Default, Clone)]
pub struct Abort {
    slot: Arc<Mutex<AbortSlot>>,
}

#[derive(Debug, Default)]
struct AbortSlot {
    stream: Option<TcpStream>,
    aborted: bool,
}

impl Abort {
    /// Close the socket in use now, and refuse new calls.
    pub fn abort(&self) {
        let mut s = self.slot.lock().unwrap();
        s.aborted = true;
        if let Some(st) = s.stream.take() {
            let _ = st.shutdown(Shutdown::Both);
        }
    }

    fn register(&self, st: &TcpStream) -> io::Result<()> {
        let mut s = self.slot.lock().unwrap();
        if s.aborted {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "sender stopped"));
        }
        s.stream = st.try_clone().ok();
        Ok(())
    }

    fn clear(&self) {
        self.slot.lock().unwrap().stream = None;
    }
}

/// Largest reply we read. The API replies are a few bytes.
const MAX_REPLY: usize = 64 * 1024;

/// POST `body` (JSON) to `ep.base + path` with the Bearer token.
pub fn post_json(
    ep: &Endpoint,
    path: &str,
    token: &str,
    body: &[u8],
    t: Timeouts,
    abort: &Abort,
) -> io::Result<Response> {
    let authority = ep.authority();
    let addrs: Vec<_> = authority.to_socket_addrs()?.collect();
    let mut last_err = io::Error::new(io::ErrorKind::NotFound, "no address for host");
    let mut stream = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, t.connect) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_err = e,
        }
    }
    let mut s = stream.ok_or(last_err)?;
    abort.register(&s)?;
    let r = exchange(&mut s, ep, &authority, path, token, body, t);
    abort.clear();
    r
}

fn exchange(
    s: &mut TcpStream,
    ep: &Endpoint,
    authority: &str,
    path: &str,
    token: &str,
    body: &[u8],
    t: Timeouts,
) -> io::Result<Response> {
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(t.io))?;
    s.set_write_timeout(Some(t.io))?;
    let head = format!(
        "POST {}{} HTTP/1.1\r\nHost: {}\r\nUser-Agent: FlightRecorder/{}\r\n\
         Authorization: Bearer {}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        ep.base,
        path,
        authority,
        crate::VERSION,
        token,
        body.len()
    );
    let mut req = Vec::with_capacity(head.len() + body.len());
    req.extend_from_slice(head.as_bytes());
    req.extend_from_slice(body);
    s.write_all(&req)?;

    let deadline = Instant::now() + t.io;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(resp) = parse_complete(&buf)? {
            return Ok(resp);
        }
        if Instant::now() > deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "reply too slow"));
        }
        let n = s.read(&mut chunk)?;
        if n == 0 {
            // EOF: the reply is what we have.
            return parse_response(&buf, true)?
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no reply"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_REPLY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "reply too large",
            ));
        }
    }
}

fn parse_complete(buf: &[u8]) -> io::Result<Option<Response>> {
    parse_response(buf, false)
}

/// Parse a reply. Without `eof`, it returns None until the head is in and
/// the body has Content-Length bytes.
fn parse_response(buf: &[u8], eof: bool) -> io::Result<Option<Response>> {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Ok(None);
    };
    let head = String::from_utf8_lossy(&buf[..end]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .filter(|_| status_line.starts_with("HTTP/1."))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad status line"))?;
    let mut len = None;
    let mut chunked = false;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "content-length" {
                len = v.parse::<usize>().ok();
            } else if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }
    let body = &buf[end + 4..];
    let body = match (len, chunked, eof) {
        (Some(n), false, _) if body.len() >= n => &body[..n],
        (Some(_), false, false) => return Ok(None),
        // Chunked or no length: the status is what matters; the body is
        // read until EOF (we sent Connection: close).
        (_, _, false) if status != 204 && status != 304 => return Ok(None),
        _ => body,
    };
    Ok(Some(Response {
        status,
        body: String::from_utf8_lossy(body).into_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn parses_replies() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}", false)
            .unwrap()
            .unwrap();
        assert_eq!((r.status, r.body.as_str()), (200, "{}"));
        assert!(
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n{}", false)
                .unwrap()
                .is_none()
        );
        assert!(parse_response(b"HTTP/1.1 401 Unauthorized\r\n", false)
            .unwrap()
            .is_none());
        let r = parse_response(b"HTTP/1.1 503 x\r\n\r\nbusy", true)
            .unwrap()
            .unwrap();
        assert_eq!((r.status, r.body.as_str()), (503, "busy"));
        assert!(parse_response(b"SSH-2.0\r\n\r\n", true).is_err());
    }

    #[test]
    fn posts_to_a_local_listener() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = Vec::new();
            let mut b = [0u8; 4096];
            while !String::from_utf8_lossy(&req).contains("\r\n\r\n{\"a\":1}") {
                let n = s.read(&mut b).unwrap();
                req.extend_from_slice(&b[..n]);
            }
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{\"ok\":true}  ")
                .unwrap();
            String::from_utf8(req).unwrap()
        });
        let ep = Endpoint::parse(&format!("http://127.0.0.1:{port}/fr")).unwrap();
        let t = Timeouts {
            connect: Duration::from_secs(1),
            io: Duration::from_secs(2),
        };
        let r = post_json(
            &ep,
            "/api/v1/events",
            "tok",
            b"{\"a\":1}",
            t,
            &Abort::default(),
        )
        .unwrap();
        assert_eq!(r.status, 200);
        let req = srv.join().unwrap();
        assert!(
            req.starts_with("POST /fr/api/v1/events HTTP/1.1\r\n"),
            "{req}"
        );
        assert!(req.contains("\r\nAuthorization: Bearer tok\r\n"));
        assert!(req.contains("\r\nContent-Length: 7\r\n"));
    }

    #[test]
    fn abort_cuts_a_blocked_read() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        // Accepts and never replies.
        let _srv = thread::spawn(move || {
            let (_s, _) = l.accept().unwrap();
            thread::sleep(Duration::from_secs(10));
        });
        let ep = Endpoint::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let abort = Abort::default();
        let a2 = abort.clone();
        let t0 = Instant::now();
        let h = thread::spawn(move || {
            let t = Timeouts {
                connect: Duration::from_secs(1),
                io: Duration::from_secs(10),
            };
            post_json(&ep, "/x", "t", b"{}", t, &a2)
        });
        thread::sleep(Duration::from_millis(100));
        abort.abort();
        let r = h.join().unwrap();
        assert!(r.is_err());
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
        // After abort, new calls fail at once.
        let ep = Endpoint::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let t = Timeouts {
            connect: Duration::from_secs(1),
            io: Duration::from_secs(1),
        };
        assert!(post_json(&ep, "/x", "t", b"{}", t, &abort).is_err());
    }
}
