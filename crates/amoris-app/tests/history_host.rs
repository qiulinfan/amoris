//! The edit history through the host (docs/spec/server.md 3.1; editor.md 8, items 1 and 5), on the
//! sailing sample served as `amoris serve` serves it: the catalog advertises `world.edit`'s
//! `group`, so the editor streams a gizmo drag; the drag's edits are one undo entry; an MCP
//! session's edit in the middle of the drag splits it; every entry names its source, and the
//! server adds the session (`mcp-<n>` as `agent` events name it, `api` for the HTTP API's client).
//! A parent destroyed through the host goes with what is under it and comes back whole in one
//! undo, one history entry.
//! And the editor's rename (editor.md 8 item 2): a `set` of `Name` whose value is the name, which
//! the catalog's schema shows, the publication carries and the history undoes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use amoris_link::Source;
use amoris_mcp::Backend as _;
use amoris_runtime::thread::{GameHandle, GameThread, Pacing, ThreadOptions};
use amoris_runtime::{GameBuilder, Project};
use amoris_server::{GameAccess, Host, McpBackend, Via};
use serde_json::{Value, json};

fn sailing() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing")
}

/// The sailing sample on a game thread started as `amoris serve` starts it (real time at speed 1,
/// paused: the edit world).
fn start() -> (GameHandle, Host) {
    let root = sailing();
    let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
    let t0 = Instant::now();
    let clock: amoris_runtime::thread::Clock =
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

/// The hierarchy through the host (server.md 3.1): the editor parents two crates to the Sloop, an
/// agent destroys the Sloop, which takes them, and one undo of the one entry brings all three
/// back under their ids, nested as before.
#[test]
fn a_parent_destroyed_through_the_host_is_undone_whole() {
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
        let under_sloop = |entity: &str| {
            json!({"set": {"entity": entity, "component": "Parent", "value": {"parent": 3}}})
        };
        call(
            Via::Editor,
            "world.edit",
            json!({"ops": [under_sloop("Crate1"), under_sloop("Crate3")], "label": "Parent crates"}),
        )
        .await;
        let sloop = |tree: &Value| -> Value {
            let nodes = tree.as_array().unwrap();
            let found = nodes.iter().find(|n| n["name"] == "Sloop");
            found.cloned().unwrap_or(Value::Null)
        };
        let before = call(Via::Editor, "world.tree", json!({})).await;
        assert_eq!(before.as_array().unwrap().len(), 7, "{before:#}");
        let children: Vec<Value> = sloop(&before)["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| json!([c["id"], c["name"]]))
            .collect();
        assert_eq!(children, [json!([4, "Crate1"]), json!([6, "Crate3"])]);
        let hash = call(Via::Api, "status", json!({})).await["world_hash"].clone();

        let agent = McpBackend::new(host.clone()).open();
        let r = agent
            .call(
                "world.edit".to_owned(),
                json!({"ops": [{"destroy": {"entity": "Sloop"}}]}),
            )
            .await
            .unwrap();
        assert_eq!(r["results"], json!([{"op": "destroyed", "count": 3}]), "{r:#}");
        let after = call(Via::Editor, "world.tree", json!({})).await;
        assert_eq!(after.as_array().unwrap().len(), 6, "{after:#}");
        assert_eq!(sloop(&after), Value::Null);
        let h = call(Via::Editor, "history.list", json!({})).await;
        assert_eq!(h["undo"], json!(["Parent crates", "destroy Sloop"]), "{h:#}");
        assert_eq!(h["entries"]["undo"][1]["session"], json!("mcp-1"), "{h:#}");

        let u = call(Via::Editor, "history.undo", json!({})).await;
        assert_eq!(u["label"], json!("destroy Sloop"), "{u:#}");
        assert_eq!(
            u["results"],
            json!([{"op": "revived", "id": 3, "count": 3}]),
            "{u:#}"
        );
        assert_eq!(call(Via::Editor, "world.tree", json!({})).await, before);
        let status = call(Via::Api, "status", json!({})).await;
        assert_eq!(status["world_hash"], hash, "{status:#}");
        let h = call(Via::Editor, "history.list", json!({})).await;
        assert_eq!(h["undo"], json!(["Parent crates"]), "{h:#}");
        assert_eq!(h["redo"], json!(["destroy Sloop"]), "{h:#}");
    });
    drop(host);
    handle.shutdown(5000).unwrap();
}

#[test]
fn a_rename_through_the_host_is_in_the_catalog_and_undoable() {
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
        // The catalog's schema says a `set` takes the fields or, for `Name`, a string.
        let catalog = call(Via::Editor, "catalog.list", json!({})).await;
        let edit = catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "world.edit")
            .unwrap();
        let defs = &edit["params"]["$defs"];
        assert_eq!(
            defs["SetOp"]["properties"]["value"]["$ref"],
            json!("#/$defs/SetValue"),
            "{edit:#}"
        );
        let forms: Vec<&str> = defs["SetValue"]["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|b| b["type"].as_str())
            .collect();
        assert_eq!(forms, ["object", "string"], "{edit:#}");
        assert!(
            defs["SetValue"]["anyOf"][1]["description"]
                .as_str()
                .is_some_and(|d| d.contains("64 bytes")),
            "{edit:#}"
        );
        assert!(
            edit["doc"].as_str().unwrap().contains("renames"),
            "{edit:#}"
        );

        // The form the editor sends for F2 and for the inspector's name field.
        let name_of = |id: u64| {
            let host = host.clone();
            async move {
                host.call(&Via::Editor, "world.get", json!({"entity": id}))
                    .await
                    .unwrap()["name"]
                    .clone()
            }
        };
        let r = call(
            Via::Editor,
            "world.edit",
            json!({"ops": [{"set": {"entity": 4, "component": "Name", "value": "CrateA"}}],
                   "label": "Rename Crate1 → CrateA"}),
        )
        .await;
        assert_eq!(r["label"], json!("Rename Crate1 → CrateA"), "{r:#}");
        let tree = call(Via::Editor, "world.tree", json!({"filter": "CrateA"})).await;
        let ids: Vec<&Value> = tree.as_array().unwrap().iter().map(|e| &e["id"]).collect();
        assert_eq!(ids, [&json!(4)], "{tree:#}");
        // The publication carries the name: `world.changed` diffs its `Name` rows.
        let published = |id: u64, name: &'static str| {
            let reader = handle.reader();
            async move {
                for _ in 0..500 {
                    let snap = reader.latest();
                    let rows = amoris_link::SnapshotView::new(&snap)
                        .rows_json("Name")
                        .unwrap();
                    if rows.contains(&(id, json!(name))) {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                false
            }
        };
        assert!(published(4, "CrateA").await);

        // An MCP session renames the same way; the history names both sources.
        let agent = McpBackend::new(host.clone()).open();
        agent
            .call(
                "world.edit".to_owned(),
                json!({"ops": [{"set": {"entity": "Crate2", "component": "Name",
                                         "value": "CrateB"}}]}),
            )
            .await
            .unwrap();
        let h = call(Via::Editor, "history.list", json!({})).await;
        let shown: Vec<Value> = h["entries"]["undo"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| json!([e["label"], e["source"].is_string(), e["session"]]))
            .collect();
        assert_eq!(
            shown,
            [
                json!(["Rename Crate1 → CrateA", true, null]),
                json!(["set Name on Crate2", false, "mcp-1"]),
            ],
            "{h:#}"
        );
        assert_eq!(h["entries"]["undo"][0]["source"], json!("editor"), "{h:#}");

        // Undo and redo through the host put the names back, in the publication too.
        call(Via::Editor, "history.undo", json!({})).await;
        assert_eq!(name_of(5).await, json!("Crate2"));
        call(Via::Editor, "history.undo", json!({})).await;
        assert_eq!(name_of(4).await, json!("Crate1"));
        assert!(published(4, "Crate1").await);
        call(Via::Editor, "history.redo", json!({})).await;
        assert_eq!(name_of(4).await, json!("CrateA"));
        assert!(published(4, "CrateA").await);

        // A removed name leaves the publication's rows, and its undo brings it back.
        call(
            Via::Editor,
            "world.edit",
            json!({"ops": [{"remove": {"entity": 6, "component": "Name"}}]}),
        )
        .await;
        assert_eq!(name_of(6).await, Value::Null);
        let unnamed = {
            let reader = handle.reader();
            async move {
                for _ in 0..500 {
                    let snap = reader.latest();
                    let rows = amoris_link::SnapshotView::new(&snap)
                        .rows_json("Name")
                        .unwrap();
                    if rows.iter().all(|(id, _)| *id != 6) {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                false
            }
        };
        assert!(unnamed.await);
        call(Via::Editor, "history.undo", json!({})).await;
        assert!(published(6, "Crate3").await);

        // A name over 64 bytes is refused at the op's value and changes nothing.
        let e = host
            .call(
                &Via::Editor,
                "world.edit",
                json!({"ops": [{"set": {"entity": 4, "component": "Name",
                                         "value": "界".repeat(22)}}]}),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, "sim.name_too_long", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/ops/0/set/value"), "{e:#?}");
        assert_eq!(e.detail["bytes"], json!(66), "{e:#?}");
        assert_eq!(name_of(4).await, json!("CrateA"));
    });
    drop(host);
    handle.shutdown(5000).unwrap();
}
