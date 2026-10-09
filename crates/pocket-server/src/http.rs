//! The HTTP side (docs/spec/host-protocol.md 1): `/api/catalog`, `/api/call`, `/ws`, `/render`
//! (the integrator's feed), `/assets/<path>`, `/mcp` and the editor's static files, behind a guard
//! that refuses any request whose `Host` or `Origin` is not loopback (DNS rebinding).

use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use pocket_contract::{Problem, detail};
use serde_json::{Value, json};

use crate::{Host, McpBackend, Via};

fn problem_response(status: StatusCode, p: &Problem) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        json!({"error": p}).to_string(),
    )
        .into_response()
}

fn json_response(v: &Value) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        v.to_string(),
    )
        .into_response()
}

/// Whether an authority (`host[:port]`) names this machine.
fn loopback(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// Refuses requests from anywhere but this machine's loopback names.
async fn guard(headers: HeaderMap, req: Request, next: Next) -> Response {
    let host_ok = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(loopback);
    let origin_ok = match headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) {
        None => true,
        Some(o) => o
            .split_once("://")
            .is_some_and(|(_, rest)| loopback(rest.split('/').next().unwrap_or(""))),
    };
    if !(host_ok && origin_ok) {
        let p = Problem::new(
            "host.forbidden",
            "This host serves loopback clients only (Host and Origin must be 127.0.0.1, \
             localhost or ::1).",
            detail([]),
        );
        return problem_response(StatusCode::FORBIDDEN, &p);
    }
    next.run(req).await
}

async fn catalog(State(host): State<Host>) -> Response {
    match host.catalog().await {
        Ok(c) => json_response(&c),
        Err(p) => problem_response(StatusCode::SERVICE_UNAVAILABLE, &p),
    }
}

/// `POST /api/call`: `{id?, method, params?, seat?}` answered `{id, result}` or `{id, error}`;
/// with `seat` the call is made as that seat's player (docs/spec/player.md), restricted by the
/// runtime to its own perception and actions.
async fn call(State(host): State<Host>, body: String) -> Response {
    let req: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            let p = Problem::new(
                "request.malformed",
                format!("The request is not JSON: {e}."),
                detail([("line", json!(e.line())), ("column", json!(e.column()))]),
            );
            return problem_response(StatusCode::BAD_REQUEST, &p);
        }
    };
    let refused = |p: Problem| json_response(&json!({"id": req.get("id"), "error": p}));
    match req.get("seat") {
        None | Some(Value::Null) => {}
        Some(Value::String(seat)) => {
            // A seat's player sends the player tools alone: anything else is refused before the
            // seat is looked up or a client opened (`seat_permits`; the game refuses it too).
            let method = req.get("method").and_then(Value::as_str);
            if let Some(Err(p)) = method.map(crate::seat_permits) {
                return refused(p);
            }
            return match host.player_via(seat).await {
                Ok(via) => json_response(&crate::ws::answer(&host, &via, req.clone()).await),
                Err(p) => refused(p),
            };
        }
        Some(other) => {
            let got = match other {
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::Array(_) => "array",
                _ => "object",
            };
            return refused(pocket_contract::codes::wrong_type(
                &pocket_contract::Pointer::root().key("seat"),
                "string",
                got,
            ));
        }
    }
    json_response(&crate::ws::answer(&host, &Via::Api, req).await)
}

async fn ws(State(host): State<Host>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| crate::ws::serve(host, socket))
}

async fn render(State(host): State<Host>, upgrade: WebSocketUpgrade) -> Response {
    let feed = host.0.render.read().ok().and_then(|f| f.clone());
    match feed {
        Some(f) => upgrade.on_upgrade(move |socket| f.serve(socket)),
        None => problem_response(
            StatusCode::NOT_IMPLEMENTED,
            &Problem::new(
                "render.not_available",
                "The render feed is not installed in this host yet.",
                detail([]),
            ),
        ),
    }
}

/// Serves `rel` from under `root`, refusing paths that leave it.
async fn file(root: &Path, rel: &str) -> Response {
    let not_found = || {
        problem_response(
            StatusCode::NOT_FOUND,
            &Problem::new(
                "host.not_found",
                format!("There is no file '{rel}'."),
                detail([("path", json!(rel))]),
            ),
        )
    };
    if !crate::local::inside(rel) {
        return not_found();
    }
    let path = root.join(rel);
    let (Ok(full), Ok(base)) = (path.canonicalize(), root.canonicalize()) else {
        return not_found();
    };
    if !full.starts_with(&base) || !full.is_file() {
        return not_found();
    }
    match tokio::fs::read(&full).await {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&full).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.essence_str().to_owned())],
                Body::from(bytes),
            )
                .into_response()
        }
        Err(_) => not_found(),
    }
}

async fn asset(
    State(host): State<Host>,
    axum::extract::Path(rel): axum::extract::Path<String>,
) -> Response {
    file(host.project(), &rel).await
}

/// The browser renderer (`web/viewport`: the editor's viewport module and its wasm package).
async fn wasm(axum::extract::Path(rel): axum::extract::Path<String>) -> Response {
    match find_web_viewport() {
        Some(dir) => file(&dir, &rel).await,
        None => (
            StatusCode::NOT_FOUND,
            "web/viewport is not built (tools/build_viewport.sh)",
        )
            .into_response(),
    }
}

/// `web/viewport` beside the working directory or the binary (or `POCKET_WEB_VIEWPORT`).
pub fn find_web_viewport() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("POCKET_WEB_VIEWPORT") {
        return Some(PathBuf::from(p));
    }
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.extend(cwd.ancestors().map(|a| a.join("web/viewport")));
    }
    if let Ok(exe) = std::env::current_exe() {
        candidates.extend(exe.ancestors().map(|a| a.join("web/viewport")));
    }
    candidates
        .into_iter()
        .find(|c| c.join("pkg/pocket_viewport_bg.wasm").is_file())
}

/// The editor's files; an unknown path gets `index.html` (the editor routes itself).
async fn editor(State(host): State<Host>, uri: Uri) -> Response {
    let Some(dist) = host.0.editor_dist.clone() else {
        return placeholder();
    };
    let rel = uri.path().trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let r = file(&dist, rel).await;
    if r.status() == StatusCode::NOT_FOUND && !rel.contains('.') {
        return file(&dist, "index.html").await;
    }
    r
}

fn placeholder() -> Response {
    let body = "<!doctype html><meta charset=utf-8><title>Amoris host</title>\
        <style>body{font:15px system-ui;margin:2rem;max-width:40rem}</style>\
        <h1>Amoris host</h1><p>The editor is not built (editor/dist). The host serves \
        <code>GET /api/catalog</code>, <code>POST /api/call</code>, <code>/ws</code>, \
        <code>/assets/&lt;path&gt;</code> and <code>POST /mcp</code>.</p>";
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

/// The server's routes over `host`; `port` is the bound port (for the MCP service's checks).
pub fn router(host: Host, port: u16) -> Router {
    let _ = port;
    let mcp = pocket_mcp::http_service(std::sync::Arc::new(McpBackend::new(host.clone())));
    Router::new()
        .route("/api/catalog", get(catalog))
        .route("/api/call", post(call))
        .route("/ws", get(ws))
        .route("/render", any(render))
        .route("/assets/{*path}", get(asset))
        .route("/wasm/{*path}", get(wasm))
        .nest_service("/mcp", mcp)
        .fallback(get(editor))
        .layer(middleware::from_fn(guard))
        .with_state(host)
}

/// The built editor, if one is found: `POCKET_EDITOR_DIST`, `editor/dist` under the working
/// directory, or `editor/dist` beside an ancestor of the executable (a workspace build).
pub fn find_editor_dist() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("POCKET_EDITOR_DIST") {
        return Some(PathBuf::from(p));
    }
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.extend(cwd.ancestors().map(|a| a.join("editor/dist")));
    }
    if let Ok(exe) = std::env::current_exe() {
        candidates.extend(exe.ancestors().map(|a| a.join("editor/dist")));
    }
    candidates
        .into_iter()
        .find(|c| c.join("index.html").is_file())
}
