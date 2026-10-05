//! A static file server on 127.0.0.1 for the web check (docs/spec/checks.md 7.3 and 7.4): the
//! wasm MIME type, `Cache-Control: no-store`, and the COOP and COEP headers only when the run is
//! cross-origin isolated, so the page is loaded both ways. The port is one the system picks.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

pub struct Server {
    pub port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(dir: &Path, isolation: bool) -> io::Result<Server> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let (dir, flag) = (dir.to_path_buf(), stop.clone());
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    let dir = dir.clone();
                    std::thread::spawn(move || {
                        let _ = respond(stream, &dir, isolation);
                    });
                }
            }
        });
        Ok(Server {
            port,
            stop,
            handle: Some(handle),
        })
    }

    pub fn url(&self, page: &str) -> String {
        format!(
            "http://127.0.0.1:{}/{}",
            self.port,
            page.trim_start_matches('/')
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port)); // wakes the accept loop
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "css" => "text/css",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "txt" | "jsonl" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// The file a request path names under `dir`, refusing anything that leaves it.
pub fn resolve(dir: &Path, target: &str) -> Option<PathBuf> {
    let path = target
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .replace("%20", " ");
    let mut out = dir.to_path_buf();
    for c in Path::new(path.trim_start_matches('/')).components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.is_dir() {
        out.push("index.html");
    }
    out.is_file().then_some(out)
}

fn respond(mut stream: TcpStream, dir: &Path, isolation: bool) -> io::Result<()> {
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 65536 {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        head.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&head);
    let mut first = text.lines().next().unwrap_or("").split_whitespace();
    let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or("/"));
    let file = if method == "GET" || method == "HEAD" {
        resolve(dir, target)
    } else {
        None
    };
    let (status, body, kind) = match file.and_then(|f| std::fs::read(&f).ok().map(|b| (b, f))) {
        Some((bytes, f)) => ("200 OK", bytes, content_type(&f)),
        None => (
            "404 Not Found",
            b"not found".to_vec(),
            "text/plain; charset=utf-8",
        ),
    };
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        body.len()
    );
    if isolation {
        response.push_str("Cross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\n");
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes())?;
    if method != "HEAD" {
        stream.write_all(&body)?;
    }
    stream.flush()?;
    stream.shutdown(Shutdown::Both)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn get(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serves_files_with_and_without_isolation() {
        let dir = std::env::temp_dir().join(format!("xtask-serve-{}", std::process::id()));
        fs::create_dir_all(dir.join("pkg")).unwrap();
        fs::write(dir.join("index.html"), "<title>x</title>").unwrap();
        fs::write(dir.join("pkg/m.wasm"), [0u8, 97, 115, 109]).unwrap();
        let isolated = Server::start(&dir, true).unwrap();
        let page = get(isolated.port, "/");
        assert!(page.starts_with("HTTP/1.1 200 OK"));
        assert!(page.contains("Cross-Origin-Embedder-Policy: require-corp"));
        assert!(page.ends_with("<title>x</title>"));
        assert!(get(isolated.port, "/pkg/m.wasm?v=1").contains("Content-Type: application/wasm"));
        assert!(get(isolated.port, "/../secret").starts_with("HTTP/1.1 404"));
        let plain = Server::start(&dir, false).unwrap();
        let page = get(plain.port, "/index.html");
        assert!(page.starts_with("HTTP/1.1 200 OK") && !page.contains("Cross-Origin"));
        drop(isolated);
        drop(plain);
        let _ = fs::remove_dir_all(dir);
    }
}
