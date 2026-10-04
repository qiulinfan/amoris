//! The editor's WebSocket (docs/spec/host-protocol.md 2 and 3): JSON text frames carrying requests
//! `{id, method, params}` answered `{id, result}` or `{id, error}` in order of completion, and the
//! pushed events `{event, data}` of the topics the socket subscribed to (`status` always).

use std::collections::BTreeSet;

use axum::extract::ws::{Message, WebSocket};
use pocket_contract::{Problem, detail};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use crate::{Host, Via};

/// The topics a socket may subscribe to.
pub const TOPICS: &[&str] = &[
    "status",
    "world.changed",
    "events",
    "log",
    "history",
    "debug",
    "profile",
    "agent",
];

/// Answers one request object: `{id, result}` or `{id, error}`.
pub async fn answer(host: &Host, via: &Via, req: Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = req.get("method").and_then(Value::as_str) else {
        let p = Problem::new(
            "request.malformed",
            "A request is {\"id\", \"method\", \"params\"}; 'method' is missing.",
            detail([]),
        );
        return json!({"id": id, "error": p});
    };
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
    match host.call(via, method, params).await {
        Ok(result) => json!({"id": id, "result": result}),
        Err(p) => json!({"id": id, "error": p}),
    }
}

fn subscribe(topics: &mut BTreeSet<String>, params: &Value, on: bool) -> Result<Value, Problem> {
    let list = params
        .get("topics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for t in &list {
        let name = t.as_str().unwrap_or("");
        if !TOPICS.contains(&name) {
            let valid: Vec<pocket_contract::Candidate<'_>> = TOPICS
                .iter()
                .map(|t| pocket_contract::Candidate::new(t))
                .collect();
            return Err(pocket_contract::codes::invalid_value(
                &pocket_contract::Pointer::root().key("topics"),
                name,
                &valid,
            ));
        }
        if on {
            topics.insert(name.to_owned());
        } else if name != "status" {
            topics.remove(name);
        }
    }
    Ok(json!({"topics": topics}))
}

/// Serves one editor socket until it closes.
pub async fn serve(host: Host, mut socket: WebSocket) {
    let mut topics: BTreeSet<String> = BTreeSet::from(["status".to_owned()]);
    let mut pushes = host.subscribe();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let first = crate::push::status_of(&host, &host.reader().latest(), 0.0);
    let hello = json!({"event": "status", "data": first}).to_string();
    if socket.send(Message::Text(hello.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            msg = socket.recv() => {
                let text = match msg {
                    Some(Ok(Message::Text(t))) => t.to_string(),
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => continue,
                };
                let req: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        let p = Problem::new(
                            "request.malformed",
                            format!("The frame is not JSON: {e}."),
                            detail([]),
                        );
                        let _ = out_tx.send(json!({"id": null, "error": p}).to_string()).await;
                        continue;
                    }
                };
                let method = req.get("method").and_then(Value::as_str).unwrap_or("");
                if method == "subscribe" || method == "unsubscribe" {
                    let id = req.get("id").cloned().unwrap_or(Value::Null);
                    let params = req.get("params").cloned().unwrap_or(Value::Null);
                    let reply = match subscribe(&mut topics, &params, method == "subscribe") {
                        Ok(r) => json!({"id": id, "result": r}),
                        Err(p) => json!({"id": id, "error": p}),
                    };
                    let _ = out_tx.send(reply.to_string()).await;
                    continue;
                }
                let (host, tx) = (host.clone(), out_tx.clone());
                tokio::spawn(async move {
                    let reply = answer(&host, &Via::Editor, req).await;
                    let _ = tx.send(reply.to_string()).await;
                });
            }
            Some(out) = out_rx.recv() => {
                if socket.send(Message::Text(out.into())).await.is_err() {
                    break;
                }
            }
            push = pushes.recv() => match push {
                Ok(p) => {
                    if topics.contains(p.topic)
                        && socket.send(Message::Text(p.frame.to_string().into())).await.is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let f = json!({"event": "lagged", "data": {"missed": n}}).to_string();
                    if socket.send(Message::Text(f.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}
