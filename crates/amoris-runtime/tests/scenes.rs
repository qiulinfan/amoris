//! Loading another scene at run time (docs/spec/simulation.md 7.6; server.md 3): `scene.load` in
//! `replace` and `add` modes with the file's places renumbered to the new ids, its refusals, a
//! script's `ctx.scene.load` applied after the tick, `scene.loaded`, and a recording that replays
//! from its own data after the file is gone.
#![cfg(feature = "transpile")]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use amoris_link::Source;
use amoris_persist::replay::{
    MemorySink, RecordOptions, Replay, ReplayMode, ReplayOutcome, VerifyOptions, verify,
};
use amoris_runtime::{Command, Game, GameBuilder, Project};
use serde_json::{Value, json};

/// A `Tag` naming another entity; at tick 3 the rule loads `scenes/two.json` when asked to by
/// the scene's `Switch`, and every `scene.loaded` is echoed as `game.seen`.
const MAIN: &str = r#"
import { component, field, game, system } from "amoris";

export const Tag = component("Tag", {
    version: 1, doc: "Names another entity.",
    fields: { on: field.entity("The entity it names.") },
});

export const Switch = component("Switch", {
    version: 1, doc: "Loads a scene at a tick.",
    fields: {
        at: field.u32(0, "The tick."),
        path: field.str("", "The scene file."),
        add: field.bool(false, "Add instead of replace."),
    },
});

export const switcher = system({
    name: "switcher", phase: "update", doc: "Loads scenes when asked.",
    events: ["scene."],
    queries: { switches: { with: ["Switch"], fields: ["Switch.at", "Switch.add"] } },
    run(ctx, { switches }) {
        for (const e of ctx.events) ctx.emit("game.seen", e.data);
        for (let r = 0; r < switches.len; r++) {
            if (switches.cols.Switch.at[r] !== ctx.tick) continue;
            const s = ctx.world.get(switches.id(r), "Switch");
            if (s) ctx.scene.load(s.path, { mode: s.add ? "add" : "replace" });
        }
    },
});

export default game({ components: [Tag, Switch], systems: [switcher] });
"#;

fn one() -> Value {
    json!([
        {"name": "Old"},
        {"name": "Keeper", "components": {"Tag": {"on": 1}}},
    ])
}

/// Three entities whose fields name each other by place: a hierarchy and a tag.
fn two() -> Value {
    json!([
        {"name": "Hall"},
        {"name": "Lamp", "components": {"Parent": {"parent": 1}, "Tag": {"on": 3}}},
        {"name": "Door", "components": {"Parent": {"parent": 1}}},
    ])
}

fn write_scene(root: &Path, rel: &str, entities: &Value) {
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": entities});
    let f = root.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, scene.to_string()).unwrap();
}

fn project(name: &str, first: &Value) -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-scenes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        format!("name = 'scenes-{name}'\nrate = 60\nseed = 1\n"),
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), MAIN).unwrap();
    write_scene(&root, "scene.json", first);
    write_scene(&root, "scenes/two.json", &two());
    write_scene(
        &root,
        "scenes/bad.json",
        &json!([{"name": "X", "components": {"Tag": {"on": 7}}}]),
    );
    root
}

fn build(root: &Path) -> Game {
    let p = Project::load(root).unwrap_or_else(|e| panic!("{e:#?}"));
    let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    GameBuilder::new(setup)
        .project(root)
        .build()
        .unwrap_or_else(|e| panic!("{e:#?}"))
}

fn try_call(g: &mut Game, name: &str, params: Value) -> Result<Value, amoris_contract::Problem> {
    g.apply(&Command::new(Source::Developer(9), 1, name, params))
}

fn call(g: &mut Game, name: &str, params: Value) -> Value {
    try_call(g, name, params).unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

/// The world's entities as `name#id`, and each `Tag.on` and `Parent.parent` by name.
fn shape(g: &mut Game) -> Vec<String> {
    fn ids(nodes: &Value, out: &mut Vec<u64>) {
        for n in nodes.as_array().into_iter().flatten() {
            out.push(n["id"].as_u64().unwrap());
            ids(&n["children"], out);
        }
    }
    let mut all = Vec::new();
    ids(&call(g, "world.tree", json!({})), &mut all);
    all.sort_unstable();
    let mut out = Vec::new();
    for id in all {
        let got = call(
            g,
            "world.get",
            json!({"entity": id, "fields": ["Tag.on", "Parent.parent"]}),
        );
        let name_of = |g: &mut Game, v: &Value| match v.as_u64() {
            Some(i) => call(g, "world.get", json!({"entity": i}))["name"]
                .as_str()
                .unwrap_or("?")
                .to_owned(),
            None => "-".into(),
        };
        let tag = name_of(g, &got["fields"]["Tag.on"]);
        let parent = name_of(g, &got["fields"]["Parent.parent"]);
        out.push(format!(
            "{}#{id} tag:{tag} parent:{parent}",
            got["name"].as_str().unwrap_or("")
        ));
    }
    out
}

#[test]
fn a_scene_replaces_or_joins_the_world_with_its_places_renumbered() {
    common::big_stack(|| {
        let root = project("modes", &one());
        let mut g = build(&root);
        g.run(2).unwrap();
        call(
            &mut g,
            "world.edit",
            json!({"ops": [{"spawn": {"name": "Extra"}}]}),
        );
        let r = call(&mut g, "scene.load", json!({"path": "scenes/two.json"}));
        assert_eq!(r["spawned"], json!(3), "{r}");
        assert_eq!(r["despawned"], json!(3), "{r}");
        assert_eq!(r["first"], json!(4), "ids go on from the allocator: {r}");
        assert_eq!(
            shape(&mut g),
            [
                "Hall#4 tag:- parent:-",
                "Lamp#5 tag:Door parent:Hall",
                "Door#6 tag:- parent:Hall"
            ]
        );
        let h = call(&mut g, "history.list", json!({}));
        assert_eq!(h["undo"], json!([]), "the load clears the history");
        // Added beside it: the same file, new ids, its own references.
        let r = call(
            &mut g,
            "scene.load",
            json!({"path": "scenes/two.json", "mode": "add"}),
        );
        assert_eq!(
            (r["spawned"].clone(), r["despawned"].clone()),
            (json!(3), json!(0))
        );
        let all = shape(&mut g);
        assert_eq!(all.len(), 6);
        assert_eq!(all[4], "Lamp#8 tag:Door parent:Hall");
        let lamp = call(
            &mut g,
            "world.get",
            json!({"entity": 8, "fields": ["Parent.parent"]}),
        );
        assert_eq!(lamp["fields"]["Parent.parent"], json!(7));
        // The next tick's systems read scene.loaded.
        let rep = g.step().unwrap();
        let seen: Vec<Value> = rep
            .events
            .iter()
            .filter(|e| e.kind.as_str() == "game.seen")
            .map(|e| e.data.to_json())
            .collect();
        assert_eq!(
            seen,
            [
                json!({"path": "scenes/two.json", "mode": "replace", "entities": 3}),
                json!({"path": "scenes/two.json", "mode": "add", "entities": 3}),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    });
}

#[test]
fn a_refused_load_changes_nothing() {
    common::big_stack(|| {
        let root = project("refused", &one());
        let mut g = build(&root);
        g.run(1).unwrap();
        let before = g.world_hash().unwrap();
        for (path, code) in [
            ("../outside.json", "request.invalid_value"),
            ("/abs.json", "request.invalid_value"),
            ("scenes/two.txt", "request.invalid_value"),
            ("scenes/none.json", "project.missing_file"),
            ("scenes/bad.json", "scene.place_invalid"),
        ] {
            let e = try_call(&mut g, "scene.load", json!({"path": path})).unwrap_err();
            assert_eq!(e.code, code, "{path}: {e:#?}");
            assert_eq!(g.world_hash().unwrap(), before, "{path}");
        }
        let e = try_call(&mut g, "scene.load", json!({"path": "scenes/bad.json"})).unwrap_err();
        assert_eq!(
            e.detail["path"],
            json!("/entities/0/components/Tag/on"),
            "{e:#?}"
        );
        // The recorded form is the host's.
        let e = try_call(
            &mut g,
            "scene_load",
            json!({"path": "scenes/two.json", "mode": "add", "scene": "00"}),
        )
        .unwrap_err();
        assert_eq!(e.code, "command.host_only");
        let _ = std::fs::remove_dir_all(&root);
    });
}

fn switching() -> Value {
    json!([
        {"name": "Old"},
        {"name": "Switch", "components": {"Switch": {"at": 3, "path": "scenes/two.json"}}},
    ])
}

/// Steps `ticks`, each tick's hash and errors' codes.
fn run(g: &mut Game, ticks: u32) -> Vec<(String, Vec<String>)> {
    (0..ticks)
        .map(|_| {
            let r = g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            let codes = r.errors.iter().map(|p| p.code.clone()).collect();
            (g.world_hash().unwrap().to_string(), codes)
        })
        .collect()
}

#[test]
fn a_script_loads_a_scene_after_its_tick_and_the_recording_replays_without_the_file() {
    common::big_stack(|| {
        let root = project("script", &switching());
        let mut g = build(&root);
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let mut fork = g.fork().unwrap();
        let first = run(&mut g, 2);
        assert!(shape(&mut g)[0].starts_with("Old#1"));
        let more = run(&mut g, 1);
        // Tick 3 asked; the boundary after it holds the new scene.
        assert_eq!(
            shape(&mut g),
            [
                "Hall#3 tag:- parent:-",
                "Lamp#4 tag:Door parent:Hall",
                "Door#5 tag:- parent:Hall"
            ]
        );
        let rest = run(&mut g, 5);
        g.finish_recording().unwrap();
        let chain: Vec<_> = first.iter().chain(&more).chain(&rest).cloned().collect();
        assert!(chain.iter().all(|(_, e)| e.is_empty()), "{chain:?}");
        // A fork taken before runs the same.
        assert_eq!(run(&mut fork, 8), chain);
        // The file goes; the recording replays from its own copy of the scene.
        std::fs::remove_file(root.join("scenes/two.json")).unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut fresh = Game::for_replay(&replay).unwrap_or_else(|e| panic!("{e:#?}"));
        let out = verify(&replay, &mut fresh, VerifyOptions::mode(ReplayMode::Verify)).outcome;
        assert_eq!(out, ReplayOutcome::Identical);
        // Asked again with the file gone, the load is refused and the world stays.
        let mut again = build(&root);
        let r = run(&mut again, 3);
        assert_eq!(r[2].1, ["project.missing_file"]);
        assert!(shape(&mut again)[0].starts_with("Old#1"));
        let _ = std::fs::remove_dir_all(&root);
    });
}
