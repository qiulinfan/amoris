//! The edit history through the host (docs/spec/server.md 3.1; editor.md 8, items 1 and 5), on the
//! sailing sample served as `pocket serve` serves it: the catalog advertises `world.edit`'s
//! `group`, so the editor streams a gizmo drag; the drag's edits are one undo entry; an MCP
//! session's edit in the middle of the drag splits it; every entry names its source, and the
//! server adds the session (`mcp-<n>` as `agent` events name it, `api` for the HTTP API's client).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use pocket_link::Source;
use pocket_mcp::Backend as _;
use pocket_runtime::thread::{GameHandle, GameThread, Pacing, ThreadOptions};
use pocket_runtime::{GameBuilder, Project};
use pocket_server::{GameAccess, Host, McpBackend, Via};
use serde_json::{Value, json};

fn sailing() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing")
}

/// The sailing sample on a game thread started as `pocket serve` starts it (real time at speed 1,
/// paused: the edit world).
fn start() -> (GameHandle, Host) {
    let root = sailing();
    let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
    let t0 = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || t0.elapsed().as_secs_f64() * 1000.0);
    let dir = root.clone();
    let handle = GameThread::spawn(
        move || GameBuilder::new(setup).project(dir).build(),
        ThreadOptions {
            pacing: Pacing::RealTime { speed: 1.0 },
            paused: true,
            ..ThreadOptions::new(clock)
        },
    )
    .unwrap();
    let clients = handle.clients();
    let access = GameAccess {
        reader: handle.reader(),
        editor: clients.client(Source::Editor).unwrap(),
        api: clients.developer(),
        developer: Arc::new(move || clients.developer()),
    };
    (handle, Host::new(access, root, None))
}

fn drag(group: &str, x: f64) -> Value {
    json!({"ops": [{"set": {"entity": "Sloop", "component": "Transform",
                             "value": {"position.0": x}}}],
           "label": "Move Sloop", "group": group})
}

#[test]
fn a_streamed_drag_is_one_entry_and_entries_name_their_sources() {
    let (handle, host) = start();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let call = |via: Via, method: &'static str, params: Value| {
            let host = host.clone();
            async move {
                host.call(&via, method, params)
                    .await
                    .unwrap_or_else(|e| panic!("{method}: {e:#?}"))
            }
        };
        let catalog = call(Via::Editor, "catalog.list", json!({})).await;
        let edit = catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "world.edit")
            .unwrap();
        assert!(
            edit["params"]["properties"]["group"].is_object(),
            "{edit:#}"
        );
        let x0 = call(
            Via::Editor,
            "world.get",
            json!({"entity": "Sloop", "fields": ["Transform.position.0"]}),
        )
        .await["fields"]["Transform.position.0"]
            .as_f64()
            .unwrap();
        for i in 1..=10 {
            call(Via::Editor, "world.edit", drag("drag-a", x0 + f64::from(i))).await;
        }
        let agent = McpBackend::new(host.clone()).open();
        agent
            .call(
                "world.edit".to_owned(),
                json!({"ops": [{"set": {"entity": "Crate1", "component": "Cargo",
                                         "value": {"value": 7}}}]}),
            )
            .await
            .unwrap();
        call(Via::Editor, "world.edit", drag("drag-a", x0 + 11.0)).await;
        call(Via::Editor, "world.edit", drag("drag-a", x0 + 12.0)).await;
        call(
            Via::Api,
            "world.edit",
            json!({"ops": [{"set": {"entity": "Crate2", "component": "Cargo",
                                     "value": {"value": 8}}}], "label": "cli edit"}),
        )
        .await;
        let h = call(Via::Editor, "history.list", json!({})).await;
        let entries = h["entries"]["undo"].as_array().unwrap();
        let shown: Vec<Value> = entries
            .iter()
            .map(|e| json!([e["label"], e["session"], e["group"]]))
            .collect();
        assert_eq!(
            shown,
            [
                json!(["Move Sloop", null, "drag-a"]),
                json!(["set Cargo on Crate1", "mcp-1", null]),
                json!(["Move Sloop", null, "drag-a"]),
                json!(["cli edit", "api", null]),
            ],
            "{h:#}"
        );
        assert_eq!(entries[0]["source"], json!("editor"));
        assert!(entries[1]["source"]["developer"].is_u64(), "{h:#}");
        assert_eq!(h["undo"].as_array().unwrap().len(), 4);
        // The MCP session's own view of the history names it too.
        let mine = agent
            .call("history.list".to_owned(), json!({}))
            .await
            .unwrap();
        assert_eq!(mine["entries"]["undo"][1]["session"], json!("mcp-1"));
        // An editor cancelling the drag asks for an undo of its group alone: refused, since the
        // drag is in two entries and the CLI's edit is next; nothing changes.
        let e = host
            .call(&Via::Editor, "history.undo", json!({"group": "drag-a"}))
            .await
            .unwrap_err();
        assert_eq!(e.code, "history.group", "{e:#?}");
        assert_eq!(e.detail["entries"], json!(2), "{e:#?}");
        let again = call(Via::Editor, "history.list", json!({})).await;
        assert_eq!(again, h);
        // Four undos take everything back: the Sloop where the drag found it.
        for _ in 0..4 {
            call(Via::Editor, "history.undo", json!({})).await;
        }
        let x = call(
            Via::Editor,
            "world.get",
            json!({"entity": "Sloop", "fields": ["Transform.position.0"]}),
        )
        .await["fields"]["Transform.position.0"]
            .as_f64()
            .unwrap();
        assert_eq!(x, x0);
        // The redo stack lists the next to redo last, as the undo stack does.
        let h = call(Via::Editor, "history.list", json!({})).await;
        assert_eq!(h["redo"][3], json!("Move Sloop"), "{h:#}");
        assert_eq!(h["entries"]["redo"][0]["session"], json!("api"), "{h:#}");
    });
    drop(host);
    handle.shutdown(5000).unwrap();
}
