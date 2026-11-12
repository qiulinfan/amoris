//! A locked host's routes (docs/spec/server.md 3.6): the routes of a host started with grants
//! (`grants.rs`), each behind an admission that takes a registered token and nothing else.
//!
//! Inside the loopback guard, every request must present a grant's token: `Authorization: Bearer
//! <token>`, or for MCP the path `/mcp/<token>` (a client that cannot set a header). A missing, a
//! malformed and an unknown token are answered alike, `401 host.unauthorized`. The grant then
//! decides what the request may be, whatever it says of itself:
//!
//! | Route | player | checker | developer |
//! |---|---|---|---|
//! | `POST /api/call`, `GET /api/grant`, `/mcp`, `/mcp/<token>` | yes | yes | yes |
//! | `GET /api/catalog`, `GET /assets/<path>` | no | yes | yes |
//! | `/ws`, `/render`, `/wasm/<path>`, the editor's files | no | no | yes |
//!
//! A route a role may not open is `403 permission.denied`. A call is the grant's session's
//! ([`Host::session_call`]): a player's is its seat's player's, the player tools alone; a
//! checker's the catalog's reads alone; a developer's everything. `/ws` and `/render` are a
//! developer's only because their requests go through the editor's client, which no gate of a
//! checker's stands before.

use std::sync::Arc;

use amoris_contract::{Pointer, Problem, codes, detail};
use axum::body::Body;
use axum::extract::{FromRef, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{any, get, post};
use axum::{Extension, Router};
use serde_json::{Value, json};

use super::{json_response, json_type, problem_response};
use crate::grants::Granted;
use crate::sessions::Role;
use crate::{Host, McpBackend};

/// What the locked routes share: the host, and one MCP service per grant, in the grants' order.
/// The services live here, in the router, never in the host: each holds the host.
#[derive(Clone)]
struct Locked {
    host: Host,
    mcp: Arc<Vec<amoris_mcp::HttpService>>,
}

impl FromRef<Locked> for Host {
    fn from_ref(locked: &Locked) -> Host {
        locked.host.clone()
    }
}

/// The token of `Authorization: Bearer <token>` (the scheme in any case), when the request has
/// exactly one such header.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut all = headers.get_all(header::AUTHORIZATION).iter();
    let (one, None) = (all.next()?, all.next()) else {
        return None;
    };
    let (scheme, token) = one.to_str().ok()?.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// The token of the path `/mcp/<token>`: exactly one segment after `/mcp/`.
fn mcp_token(path: &str) -> Option<&str> {
    path.strip_prefix("/mcp/")
        .filter(|t| !t.is_empty() && !t.contains('/'))
}

/// The token a request presents: its bearer, or the segment of `/mcp/<token>`. A request that
/// gives both must give the same one; one that gives two different ones presents nothing. A
/// token in the query or in the body is never looked at.
fn presented<'a>(headers: &'a HeaderMap, path: &'a str) -> Option<&'a str> {
    match (bearer(headers), mcp_token(path)) {
        (Some(sent), Some(segment)) => sent.eq_ignore_ascii_case(segment).then_some(sent),
        (Some(token), None) | (None, Some(token)) => Some(token),
        (None, None) => None,
    }
}

/// `401 host.unauthorized`: the one answer to a request with no token, a malformed one or one no
/// grant has. It says nothing of what was presented.
fn unauthorized() -> Response {
    let p = Problem::new(
        "host.unauthorized",
        "This host is locked: a request needs the token of one of its grants, as the header \
         'Authorization: Bearer <token>' (MCP also at /mcp/<token>). The amoris CLI takes \
         --token or AMORIS_TOKEN.",
        detail([]),
    );
    let mut r = problem_response(StatusCode::UNAUTHORIZED, &p);
    r.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_static("Bearer"),
    );
    r
}

/// Whether `path` is a file under `prefix`: the prefix and a name after it, as the router's
/// `<prefix>{*path}` matches (the prefix alone is the editor's fallback).
fn under(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| !rest.is_empty())
}

/// The route a path is, as a refusal names it (never the path itself, which may hold a token),
/// and the least role that opens it. What no route names is the editor's, a developer's.
fn route_of(path: &str) -> (&'static str, Role) {
    match path {
        "/api/call" => ("/api/call", Role::Player),
        "/api/grant" => ("/api/grant", Role::Player),
        "/mcp" => ("/mcp", Role::Player),
        "/api/catalog" => ("/api/catalog", Role::Checker),
        "/ws" => ("/ws", Role::Developer),
        "/render" => ("/render", Role::Developer),
        p if mcp_token(p).is_some() => ("/mcp", Role::Player),
        p if under(p, "/assets/") => ("/assets", Role::Checker),
        p if under(p, "/wasm/") => ("/wasm", Role::Developer),
        _ => ("/", Role::Developer),
    }
}

/// How much a role opens: each opens what the one before it does.
fn rank(role: Role) -> u8 {
    match role {
        Role::Player => 0,
        Role::Checker => 1,
        Role::Developer => 2,
    }
}

/// Whether `role` may open the route of `path`: deny unless the table above says so.
fn opens(method: &Method, path: &str, role: Role) -> Result<(), Problem> {
    let (route, needs) = route_of(path);
    if rank(role) >= rank(needs) {
        Ok(())
    } else {
        Err(codes::permission_denied(
            &format!("{method} {route}"),
            role.name(),
            needs.name(),
        ))
    }
}

/// The admission, inside the loopback guard: no grant, no answer but the 401; a route the
/// grant's role may not open, the 403. A request it lets through carries its grant.
async fn admit(State(host): State<Host>, mut req: Request, next: Next) -> Response {
    let who = presented(req.headers(), req.uri().path()).and_then(|token| host.granted(token));
    let Some(who) = who else {
        return unauthorized();
    };
    if let Err(p) = opens(req.method(), req.uri().path(), who.role) {
        return problem_response(StatusCode::FORBIDDEN, &p);
    }
    req.extensions_mut().insert(who);
    next.run(req).await
}

/// `GET /api/grant`: what the presented token is, `{role, seat, label, session}`. Answered
/// without the game, so a client learns its grant while the debugger holds the host.
async fn grant(Extension(who): Extension<Granted>) -> Response {
    json_response(
        &json!({"role": who.role.name(), "seat": who.seat, "label": who.label,
                          "session": who.session}),
    )
}

/// `POST /api/call` on a locked host: the call of the grant's session, whatever the body says.
/// A player's `seat` can only be its own and it has no `handle`; a checker names neither; a
/// developer names a seat or a session of its own as on a host without grants.
async fn call(
    State(host): State<Host>,
    Extension(who): Extension<Granted>,
    body: String,
) -> Response {
    let Ok(req) = serde_json::from_str::<Value>(&body) else {
        // Not JSON: nothing in it is called, and the answer is the unlocked host's 400.
        return super::call(State(host), body).await;
    };
    let given = |key: &str| req.get(key).filter(|v| !v.is_null());
    let (seat, handle) = (given("seat"), given("handle"));
    if who.role == Role::Developer && (seat.is_some() || handle.is_some()) {
        return super::call(State(host), body).await;
    }
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(Value::as_str);
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
    // A refusal of the request's envelope: the grant's failed call, when it names a method.
    let refused = |p: Problem| {
        let p = match method {
            Some(m) => host.agent_refused(&who.session, m, &params, p),
            None => p,
        };
        json_response(&json!({"id": id, "error": p}))
    };
    let role = who.role.name();
    if handle.is_some() {
        return refused(codes::permission_denied(
            "POST /api/call with 'handle'",
            role,
            "developer",
        ));
    }
    match (seat, who.seat.as_deref()) {
        (None, _) => {}
        // A player's own seat says nothing new; another is refused before any seat is looked up.
        (Some(Value::String(seat)), Some(own)) if seat == own => {}
        (Some(Value::String(seat)), Some(own)) => {
            return refused(codes::seat_not_yours(
                &Pointer::root().key("seat"),
                seat,
                own,
            ));
        }
        (Some(other), Some(_)) => {
            return refused(codes::wrong_type(
                &Pointer::root().key("seat"),
                "string",
                json_type(other),
            ));
        }
        (Some(_), None) => {
            return refused(codes::permission_denied(
                "POST /api/call with 'seat'",
                role,
                "developer",
            ));
        }
    }
    let Some(method) = method else {
        let p = Problem::new(
            "request.malformed",
            "A request is {\"id\", \"method\", \"params\"}; 'method' is missing.",
            detail([]),
        );
        return json_response(&json!({"id": id, "error": p}));
    };
    match host.session_call(&who.session, method, params).await {
        Ok(result) => json_response(&json!({"id": id, "result": result})),
        Err(p) => json_response(&json!({"id": id, "error": p})),
    }
}

/// `/mcp` and `/mcp/<token>`: the MCP service of the grant, which has sessions of its own, so
/// one grant's `Mcp-Session-Id` is no session under another's token. The service is handed
/// neither the token's header nor its path.
async fn mcp(
    State(locked): State<Locked>,
    Extension(who): Extension<Granted>,
    mut req: Request,
) -> Response {
    req.headers_mut().remove(header::AUTHORIZATION);
    *req.uri_mut() = Uri::from_static("/mcp");
    match locked.mcp.get(who.index) {
        Some(service) => service.handle(req).await.map(Body::new),
        None => problem_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &codes::internal_error("the locked host's MCP route", "a grant with no MCP service"),
        ),
    }
}

/// The locked host's routes over `host`.
pub(super) fn router(host: Host) -> Router {
    let mcp_services = host
        .grants()
        .iter()
        .map(|g| {
            let backend = McpBackend::granted(host.clone(), &g.session, g.seat.as_deref());
            amoris_mcp::http_service(Arc::new(backend))
        })
        .collect();
    let locked = Locked {
        host: host.clone(),
        mcp: Arc::new(mcp_services),
    };
    Router::new()
        .route("/api/call", post(call))
        .route("/api/grant", get(grant))
        .route("/api/catalog", get(super::catalog))
        .route("/ws", get(super::ws))
        .route("/render", any(super::render))
        .route("/assets/{*path}", get(super::asset))
        .route("/wasm/{*path}", get(super::wasm))
        .route("/mcp", any(mcp))
        .route("/mcp/{token}", any(mcp))
        .fallback(get(super::editor))
        .layer(middleware::from_fn_with_state(host, admit))
        .layer(middleware::from_fn(super::guard))
        .with_state(locked)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210";

    fn headers(authorization: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for a in authorization {
            h.append(header::AUTHORIZATION, a.parse().unwrap());
        }
        h
    }

    #[test]
    fn a_token_is_a_bearer_or_the_mcp_paths_segment() {
        let none = headers(&[]);
        // The header, its scheme in any case; another scheme, or none, is no token.
        for a in ["Bearer ", "bearer ", "BEARER ", "Bearer   "] {
            let h = headers(&[&format!("{a}{TOKEN}")]);
            assert_eq!(presented(&h, "/api/call"), Some(TOKEN), "{a:?}");
        }
        for a in [
            format!("Basic {TOKEN}"),
            TOKEN.to_owned(),
            format!("Bearer{TOKEN}"),
            format!("Token {TOKEN}"),
        ] {
            assert_eq!(presented(&headers(&[&a]), "/api/call"), None, "{a:?}");
        }
        // Two headers are no token, even when they agree.
        let twice = format!("Bearer {TOKEN}");
        assert_eq!(presented(&headers(&[&twice, &twice]), "/mcp"), None);
        // The path, for MCP alone, and only as one whole segment.
        assert_eq!(presented(&none, &format!("/mcp/{TOKEN}")), Some(TOKEN));
        for path in [
            "/mcp".to_owned(),
            "/mcp/".to_owned(),
            format!("/mcp/{TOKEN}/"),
            format!("/mcp/a/{TOKEN}"),
            format!("/api/call/{TOKEN}"),
            format!("/assets/{TOKEN}"),
            format!("/{TOKEN}"),
        ] {
            assert_eq!(presented(&none, &path), None, "{path}");
        }
        // Both: the same token (in either case), or nothing.
        let h = headers(&[&format!("Bearer {TOKEN}")]);
        assert_eq!(presented(&h, &format!("/mcp/{TOKEN}")), Some(TOKEN));
        assert_eq!(
            presented(&h, &format!("/mcp/{}", TOKEN.to_uppercase())),
            Some(TOKEN)
        );
        assert_eq!(presented(&h, &format!("/mcp/{OTHER}")), None);
    }

    #[test]
    fn a_route_opens_to_the_roles_the_table_names() {
        let mcp = format!("/mcp/{TOKEN}");
        let all = [Role::Player, Role::Checker, Role::Developer];
        let table: [(&str, &str, &[Role]); 16] = [
            ("/api/call", "/api/call", &all[..]),
            ("/api/grant", "/api/grant", &all[..]),
            ("/mcp", "/mcp", &all[..]),
            (mcp.as_str(), "/mcp", &all[..]),
            ("/api/catalog", "/api/catalog", &all[1..]),
            ("/assets/scene.json", "/assets", &all[1..]),
            ("/ws", "/ws", &all[2..]),
            ("/render", "/render", &all[2..]),
            ("/wasm/pkg/amoris_viewport.js", "/wasm", &all[2..]),
            ("/", "/", &all[2..]),
            ("/index.html", "/", &all[2..]),
            // What only looks like a route is the editor's.
            ("/api/catalog/x", "/", &all[2..]),
            ("/assets", "/", &all[2..]),
            ("/assets/", "/", &all[2..]),
            ("/api/call/", "/", &all[2..]),
            ("/mcp/a/b", "/", &all[2..]),
        ];
        for (path, named, open_to) in table {
            for role in all {
                let r = opens(&Method::GET, path, role);
                if open_to.contains(&role) {
                    assert!(r.is_ok(), "{path} as a {}", role.name());
                    continue;
                }
                let p = r.unwrap_err();
                assert_eq!(p.code, "permission.denied", "{path}: {p:#?}");
                assert_eq!(p.detail["request"], json!(format!("GET {named}")), "{p:#?}");
                assert_eq!(p.detail["role"], json!(role.name()), "{p:#?}");
                // The least role that opens it.
                assert_eq!(p.detail["needs"], json!(open_to[0].name()), "{p:#?}");
                assert!(!format!("{p:?}").contains(TOKEN), "{p:#?}");
            }
        }
    }

    #[test]
    fn the_refusal_of_no_token_says_nothing_of_the_request() {
        let r = unauthorized();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(r.headers().get(header::WWW_AUTHENTICATE).unwrap(), "Bearer");
    }
}
