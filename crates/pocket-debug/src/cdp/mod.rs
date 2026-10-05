//! The Chrome DevTools Protocol endpoint (host-protocol.md 6; docs/spec/debugger.md 4): HTTP
//! discovery (`/json/list`, `/json/version`, `/json`) and WebSocket sessions on
//! `ws://127.0.0.1:<port>/devtools/game`, one thread each, for Chrome DevTools and VS Code's
//! js-debug (a `node` attach). Requests whose `Host` or `Origin` is not loopback are refused (DNS
//! rebinding).
//!
//! The endpoint never touches the game: sessions edit the hub and ask the stopped game thread
//! through it.

mod session;

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

use crate::hub::DebugHub;

/// The WebSocket path of the one target.
pub const TARGET_PATH: &str = "/devtools/game";
/// The target's id.
pub const TARGET_ID: &str = "pocket-game";

/// Where the endpoint listens.
#[derive(Clone, Debug)]
pub struct CdpOptions {
    /// Default 9229 (Node's inspector port, where VS Code looks first); 0 picks a free port.
    pub port: u16,
    /// Default 127.0.0.1. Anything but a loopback address is refused.
    pub address: IpAddr,
    /// Log every CDP message on stderr.
    pub log: bool,
}

impl Default for CdpOptions {
    fn default() -> CdpOptions {
        CdpOptions {
            port: 9229,
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            log: std::env::var_os("POCKET_CDP_LOG").is_some(),
        }
    }
}

/// A running endpoint; dropping it (or [`CdpServer::stop`]) stops accepting and closes nothing
/// already open (sessions end with their clients).
pub struct CdpServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    hub: DebugHub,
}

impl CdpServer {
    /// The bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `ws://127.0.0.1:<port>/devtools/game`.
    pub fn ws_url(&self) -> String {
        format!("ws://{}{TARGET_PATH}", self.addr)
    }

    /// The URL to open in Chrome to debug with DevTools.
    pub fn devtools_url(&self) -> String {
        devtools_url(&self.addr)
    }

    /// Stops accepting connections.
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Release);
        crate::hub::lock(&self.hub.inner.cdp).take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for CdpServer {
    fn drop(&mut self) {
        self.halt();
    }
}

pub(crate) fn devtools_url(addr: &SocketAddr) -> String {
    format!(
        "devtools://devtools/bundled/js_app.html?experiments=true&v8only=true&ws={addr}{TARGET_PATH}"
    )
}

impl DebugHub {
    /// Serves CDP on `options.address:options.port` from a thread of its own.
    pub fn serve_cdp(&self, options: CdpOptions) -> std::io::Result<CdpServer> {
        if !options.address.is_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the CDP endpoint listens on a loopback address only",
            ));
        }
        let listener = TcpListener::bind((options.address, options.port))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let hub = self.clone();
        let thread = std::thread::Builder::new()
            .name("pocket-cdp".into())
            .spawn(move || accept(listener, addr, hub, flag, options.log))?;
        *crate::hub::lock(&self.inner.cdp) = Some(addr);
        Ok(CdpServer {
            addr,
            stop,
            thread: Some(thread),
            hub: self.clone(),
        })
    }
}

fn accept(
    listener: TcpListener,
    addr: SocketAddr,
    hub: DebugHub,
    stop: Arc<AtomicBool>,
    log: bool,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let hub = hub.clone();
                let _ = std::thread::Builder::new()
                    .name("pocket-cdp-conn".into())
                    .spawn(move || connection(stream, addr, hub, log));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// The request head, peeked (the WebSocket handshake reads it again).
fn peek_head(stream: &TcpStream) -> Option<String> {
    let mut buf = [0u8; 8192];
    let deadline = Instant::now() + Duration::from_secs(2);
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok()?;
    loop {
        let n = stream.peek(&mut buf).ok()?;
        let head = String::from_utf8_lossy(&buf[..n]).to_string();
        if let Some(end) = head.find("\r\n\r\n") {
            let _ = stream.set_read_timeout(None);
            return Some(head[..end + 4].to_owned());
        }
        if n == buf.len() || Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// A loopback host name, with or without a port.
fn loopback_host(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map_or(host, |(h, p)| {
            if p.chars().all(|c| c.is_ascii_digit()) {
                h
            } else {
                host
            }
        })
    };
    matches!(name, "127.0.0.1" | "localhost" | "::1")
        || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether a request's `Host` and `Origin` are loopback (or DevTools' own origin).
fn allowed(head: &str) -> bool {
    let host_ok = header(head, "host").is_none_or(loopback_host);
    let origin_ok = header(head, "origin").is_none_or(|o| {
        let o = o.trim_end_matches('/');
        if o.starts_with("devtools://") || o.starts_with("chrome-devtools://") || o == "null" {
            return true;
        }
        ["http://", "https://"]
            .iter()
            .find_map(|scheme| o.strip_prefix(scheme))
            .is_some_and(loopback_host)
    });
    host_ok && origin_ok
}

fn connection(stream: TcpStream, addr: SocketAddr, hub: DebugHub, log: bool) {
    let _ = stream.set_nonblocking(false);
    let Some(head) = peek_head(&stream) else {
        return;
    };
    let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let upgrade = header(&head, "upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    if !allowed(&head) {
        respond(
            stream,
            &head,
            "403 Forbidden",
            "text/plain",
            "Only loopback hosts and origins may debug this game.",
        );
        return;
    }
    if upgrade {
        let path_only = path.split('?').next().unwrap_or("");
        if path_only != TARGET_PATH && path_only != format!("/devtools/{TARGET_ID}") {
            respond(
                stream,
                &head,
                "404 Not Found",
                "text/plain",
                "No such target.",
            );
            return;
        }
        match tungstenite::accept(stream) {
            Ok(ws) => session::Session::new(hub, log).run(ws),
            Err(e) => eprintln!("pocket-cdp: handshake failed: {e}"),
        }
        return;
    }
    let ws = format!("{addr}{TARGET_PATH}");
    let body: Option<Json> = match path.split('?').next().unwrap_or("") {
        "/json" | "/json/list" | "/json/list/" => Some(json!([{
            "description": "Pocket3D game thread (QuickJS-ng)",
            "devtoolsFrontendUrl": devtools_url(&addr),
            "devtoolsFrontendUrlCompat": format!("devtools://devtools/bundled/inspector.html?experiments=true&v8only=true&ws={ws}"),
            "faviconUrl": "",
            "id": TARGET_ID,
            "title": hub.inner.title,
            "type": "node",
            "url": "pocket:///scripts/main.js",
            "webSocketDebuggerUrl": format!("ws://{ws}"),
        }])),
        // As Node answers: no webSocketDebuggerUrl here, or js-debug takes the target for a browser.
        "/json/version" => {
            Some(json!({"Browser": "Pocket3D/0.1 (QuickJS-ng 0.16.2)", "Protocol-Version": "1.3"}))
        }
        "/json/protocol" => Some(json!({"version": {"major": "1", "minor": "3"}, "domains": []})),
        _ => None,
    };
    match body {
        Some(b) => respond(
            stream,
            &head,
            "200 OK",
            "application/json; charset=UTF-8",
            &b.to_string(),
        ),
        None => respond(stream, &head, "404 Not Found", "text/plain", "Not found."),
    }
}

fn respond(mut stream: TcpStream, head: &str, status: &str, kind: &str, body: &str) {
    // Read the request before answering: closing a socket with unread bytes resets it.
    let mut request = vec![0u8; head.len()];
    let _ = stream.read_exact(&mut request);
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_only() {
        let req = |host: &str, origin: Option<&str>| {
            let mut h = format!("GET /json HTTP/1.1\r\nHost: {host}\r\n");
            if let Some(o) = origin {
                h.push_str(&format!("Origin: {o}\r\n"));
            }
            h.push_str("\r\n");
            allowed(&h)
        };
        assert!(req("127.0.0.1:9229", None));
        assert!(req("localhost:9229", Some("devtools://devtools")));
        assert!(req("[::1]:9229", Some("http://127.0.0.1:7878")));
        assert!(!req("evil.example:9229", None));
        assert!(!req("127.0.0.1:9229", Some("http://evil.example")));
        assert!(!req("127.0.0.1.evil.example", None));
    }
}
