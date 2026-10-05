//! The host protocol's commands on the game (docs/spec/server.md): `world.edit` with undo and redo
//! (a destroy comes back under its id with every component), the readers (`world.tree`,
//! `world.query`, `world.schema`), `time.step` with stop conditions, and on the game thread Play,
//! Stop and the kept snapshots.
#![cfg(all(feature = "thread", feature = "transpile"))]
#![allow(clippy::disallowed_methods)]
mod common;

use std::sync::Arc;
use std::time::Instant;

use pocket_link::Source;
use pocket_runtime::thread::{GameThread, ThreadOptions};
use pocket_runtime::{Command, Game};
use serde_json::{Value, json};

fn call(g: &mut Game, name: &str, params: Value) -> Result<Value, pocket_contract::Problem> {
    g.apply(&Command::new(Source::Editor, 1, name, params))
}

fn ok(g: &mut Game, name: &str, params: Value) -> Value {
    call(g, name, params).unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

#[test]
fn edits_undo_and_redo_exactly() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let start = g.world_hash().unwrap();
        let r = ok(
            &mut g,
            "world.edit",
            json!({"ops": [
                {"set": {"entity": "Crate1", "component": "Cargo", "value": {"value": 9}}},
                {"spawn": {"name": "Extra", "prefab": {"kind": "crate", "position": [1, 0, 1]}}},
                {"destroy": {"entity": "Crate2"}},
            ]}),
        );
        assert_eq!(r["applied"], json!(3), "{r}");
        assert!(r["label"].as_str().unwrap().contains("Cargo"), "{r}");
        let h = ok(&mut g, "history.list", json!({}));
        assert_eq!(h["undo"].as_array().unwrap().len(), 1, "{h}");
        let edited = g.world_hash().unwrap();
        assert_ne!(edited, start);
        let u = ok(&mut g, "history.undo", json!({}));
        assert_eq!(u["label"], r["label"]);
        // The spawned crate's id stays allocated, so the allocator differs; the entities and their
        // components are back as they were.
        let crate2 = ok(&mut g, "world.get", json!({"entity": "Crate2"}));
        assert_eq!(crate2["id"], json!(5), "{crate2}");
        assert_eq!(crate2["components"]["Cargo"]["value"], json!(2));
        let crate1 = ok(
            &mut g,
            "world.get",
            json!({"entity": "Crate1", "components": ["Cargo"]}),
        );
        assert_eq!(crate1["components"]["Cargo"]["value"], json!(1));
        assert!(call(&mut g, "world.get", json!({"entity": "Extra"})).is_err());
        ok(&mut g, "history.redo", json!({}));
        let crate1 = ok(
            &mut g,
            "world.get",
            json!({"entity": "Crate1", "components": ["Cargo"]}),
        );
        assert_eq!(crate1["components"]["Cargo"]["value"], json!(9));
        assert!(call(&mut g, "world.get", json!({"entity": "Crate2"})).is_err());
        let e = call(&mut g, "history.redo", json!({})).unwrap_err();
        assert_eq!(e.code, "history.empty");
        // A typo is refused with a suggestion and changes nothing.
        let e = call(
            &mut g,
            "world.edit",
            json!({"ops": [{"set": {"entity": "Sloop", "component": "Boat", "value": {}, "replce": true}}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, "request.unknown_field", "{e:#?}");
        // A restore clears the history.
        let snap = g.snapshot().unwrap();
        g.restore(&snap).unwrap();
        let h = ok(&mut g, "history.list", json!({}));
        assert_eq!(h, json!({"undo": [], "redo": []}));
    });
}

#[test]
fn readers_and_stop_conditions() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let tree = ok(&mut g, "world.tree", json!({"filter": "crate"}));
        assert_eq!(tree.as_array().unwrap().len(), 4, "{tree}");
        assert!(
            tree[0]["components"]
                .as_array()
                .unwrap()
                .contains(&json!("Cargo"))
        );
        let rows = ok(
            &mut g,
            "world.query",
            json!({"with": ["Boat"], "fields": ["Boat.speed", "Transform.position.x"]}),
        );
        assert_eq!(rows[0]["name"], json!("Sloop"), "{rows}");
        assert!(rows[0]["Transform.position.x"].is_number(), "{rows}");
        let s = ok(&mut g, "world.schema", json!({"component": "Cargo"}));
        assert_eq!(s["name"], json!("Cargo"));
        let e = call(&mut g, "world.schema", json!({"component": "Cargo2"})).unwrap_err();
        assert_eq!(e.code, "sim.component_unknown");
        let r = ok(
            &mut g,
            "time.step",
            json!({"ticks": 600, "until": {"event": "sail.*"}}),
        );
        assert_eq!(r["stopped_by"]["reason"], json!("event"), "{r}");
        let r = ok(
            &mut g,
            "time.step",
            json!({"ticks": 600, "watch": {"entity": "Sloop", "component": "Boat",
                                           "field": "speed", "op": ">", "value": 0.5}}),
        );
        assert_eq!(r["stopped_by"]["reason"], json!("watch"), "{r}");
        assert!(r["stopped_by"]["value"].as_f64().unwrap() > 0.5);
        let e = call(&mut g, "play.start", json!({})).unwrap_err();
        assert_eq!(e.code, "command.thread_only");
    });
}

#[test]
fn play_stop_and_kept_snapshots_on_the_thread() {
    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let h = GameThread::spawn(
        || Game::new(common::sailing(), 1),
        ThreadOptions::new(clock),
    )
    .unwrap();
    let reader = h.reader();
    let mut dev = h.developer();
    let mut c = |n: &str, p: Value| {
        dev.call(n, p)
            .unwrap_or_else(|e| panic!("{n}: {e:#?}"))
            .into_json()
    };
    c("time.step", json!({"ticks": 130}));
    let kept = c("snapshots.list", json!({}));
    let ticks: Vec<u64> = kept["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["tick"].as_u64().unwrap())
        .collect();
    assert_eq!(ticks, [0, 60, 120]);
    let edit_hash = c("status", json!({}))["world_hash"].clone();
    let s = c("play.start", json!({"speed": 50}));
    assert_eq!(s["mode"], json!("play"), "{s}");
    assert_eq!(reader.world().mode, pocket_link::WorldMode::Play);
    std::thread::sleep(std::time::Duration::from_millis(200));
    c(
        "world.edit",
        json!({"ops": [{"destroy": {"entity": "Crate4"}}]}),
    );
    let s = c("play.stop", json!({}));
    assert_eq!(s["mode"], json!("edit"));
    assert_eq!(s["tick"], json!(130), "{s}");
    assert_eq!(s["world_hash"], edit_hash);
    assert!(
        c("world.tree", json!({"filter": "Crate4"}))
            .as_array()
            .unwrap()
            .len()
            == 1
    );
    let r = c("snapshots.restore", json!({"tick": 100}));
    assert_eq!(r["restored"], json!(60), "{r}");
    assert_eq!(r["tick"], json!(60));
    drop(dev);
    h.shutdown(2000).unwrap();
}

#[test]
fn script_types_declare_engine_and_game_components() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap();
        let r = ok(&mut g, "scripts.types", json!({"text": true}));
        let names = |k: &str| r["components"][k].as_array().unwrap().clone();
        assert!(names("engine").contains(&json!("Boat")), "{r}");
        assert!(names("engine").contains(&json!("Transform")), "{r}");
        assert_eq!(
            names("project"),
            vec![json!("Crew"), json!("Tally"), json!("Log"), json!("Cargo")]
        );
        assert!(names("unavailable").contains(&json!("Model")), "{r}");
        assert_eq!(r["project_from"], json!("scripts"), "{r}");
        // A game built from data has no project directory: the text only.
        assert_eq!(r["dir"], Value::Null);
        let components = r["text"]["components.d.ts"].as_str().unwrap();
        assert!(components.contains("declare module \"pocket\""));
        assert!(
            components.contains("readonly take: Entity | null;"),
            "{components}"
        );
        assert!(
            components.contains("readonly take: EntityColumn;"),
            "{components}"
        );
        assert!(
            components.contains("readonly position: Vec3Columns;"),
            "{components}"
        );
        let pocket = r["text"]["pocket.d.ts"].as_str().unwrap();
        assert!(pocket.starts_with("/// <reference path=\"./components.d.ts\" />"));
        assert!(pocket.contains("export declare function system<"));
    });
}

fn copy_project(from: &std::path::Path, to: &std::path::Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to.join("scripts")).unwrap();
    for f in ["project.toml", "scene.json"] {
        std::fs::copy(from.join(f), to.join(f)).unwrap();
    }
    for e in std::fs::read_dir(from.join("scripts")).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), to.join("scripts").join(e.file_name())).unwrap();
    }
}

fn game_in(dir: &std::path::Path) -> Game {
    let setup = pocket_runtime::Project::load(dir)
        .and_then(|p| p.setup(false))
        .unwrap_or_else(|e| panic!("{e:#?}"));
    pocket_runtime::GameBuilder::new(Arc::new(setup))
        .project(dir)
        .build()
        .unwrap_or_else(|e| panic!("{e:#?}"))
}

fn edit(path: &std::path::Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{} lacks {from}", path.display());
    std::fs::write(path, text.replacen(from, to, 1)).unwrap();
}

/// script-host.md 7.4: `scripts.apply {types}` writes the declarations of what it compiled under
/// `.pocket/types/` only (with the `tsconfig.json` tsc then uses), a component the scripts on disk
/// add is declared before any swap, and scripts that compile but would not load refuse the dry run
/// as the swap would; only an explicit `scripts.types` creates the project's `tsconfig.json`.
#[test]
fn declarations_follow_the_scripts_and_stay_out_of_the_source_tree() {
    common::big_stack(|| {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("types-project");
        copy_project(&common::repo().join("samples/sailing"), &dir);
        let mut g = game_in(&dir);
        let r = ok(
            &mut g,
            "scripts.apply",
            json!({"dry_run": true, "types": true}),
        );
        assert_eq!(r["outcome"], json!("unchanged"), "{r}");
        assert_eq!(r["types"]["project_from"], json!("scripts"), "{r}");
        assert_eq!(
            r["types"]["components"]["project"],
            json!(["Crew", "Tally", "Log", "Cargo"]),
            "{r}"
        );
        assert_eq!(r["types"]["tsconfig"], json!("none"), "{r}");
        assert!(!dir.join("tsconfig.json").exists());
        assert!(dir.join(".pocket/types/tsconfig.json").is_file());

        let components = dir.join("scripts/components.ts");
        edit(
            &components,
            "worth: field.u32(0,",
            "bonus: field.u32(0, \"A bonus.\"),\n        worth: field.u32(0,",
        );
        let r = ok(
            &mut g,
            "scripts.apply",
            json!({"dry_run": true, "types": true}),
        );
        assert_eq!(r["outcome"], json!("dry_run"), "{r}");
        let dts = std::fs::read_to_string(dir.join(".pocket/types/components.d.ts")).unwrap();
        assert!(dts.contains("readonly bonus: number;"), "{dts}");

        edit(
            &dir.join("scripts/rules.ts"),
            "with: [\"Tally\"]",
            "with: [\"Tallie\"]",
        );
        let e = call(
            &mut g,
            "scripts.apply",
            json!({"dry_run": true, "types": true}),
        )
        .unwrap_err();
        assert_eq!(e.code, "scripts.refused", "{e:#?}");
        assert!(
            e.detail["diagnostics"]
                .as_array()
                .is_some_and(|d| !d.is_empty()),
            "{e:#?}"
        );
        assert_eq!(
            e.detail["types"]["project_from"],
            json!("registry"),
            "{e:#?}"
        );

        let t = ok(&mut g, "scripts.types", json!({}));
        assert_eq!(t["tsconfig"], json!("created"), "{t}");
        assert!(dir.join("tsconfig.json").is_file());
    });
}

/// The game-thread cost of the declarations (docs/bench/typecheck.md), on sailing and on a
/// generated project of 40 components and 40 systems: `scripts.types` with the scripts unchanged
/// (one compile; the running program answers the components) and `scripts.apply {dry_run, types}`
/// on changed scripts (compile, instantiate in a throwaway host, emit, write): what a
/// `scripts.check` costs the game thread. Median of 15 calls each.
/// `cargo test --release -p pocket-runtime --features thread,transpile --test host
/// script_types_cost -- --ignored --nocapture`
#[test]
#[ignore]
fn script_types_cost() {
    common::big_stack(|| {
        let tmp = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
        let sailing = tmp.join("types-cost-sailing");
        copy_project(&common::repo().join("samples/sailing"), &sailing);
        let big = tmp.join("types-cost-big");
        generate(&big, 40, 8, 40);
        for (name, dir) in [("sailing", &sailing), ("generated 40x8, 40 systems", &big)] {
            let mut g = game_in(dir);
            let median = |g: &mut Game, f: &mut dyn FnMut(&mut Game, usize)| {
                let mut t: Vec<f64> = (0..15)
                    .map(|i| {
                        let s = Instant::now();
                        f(g, i);
                        s.elapsed().as_secs_f64() * 1000.0
                    })
                    .collect();
                t.sort_by(f64::total_cmp);
                (t[7], t[0], t[14])
            };
            let unchanged = median(&mut g, &mut |g, _| {
                ok(g, "scripts.types", json!({"tsconfig": false}));
            });
            let main = dir.join("scripts/main.ts");
            let text = std::fs::read_to_string(&main).unwrap();
            let changed = median(&mut g, &mut |g, i| {
                std::fs::write(&main, format!("{text}// {i}\n")).unwrap();
                let r = ok(g, "scripts.apply", json!({"dry_run": true, "types": true}));
                assert_eq!(r["outcome"], json!("dry_run"), "{r}");
            });
            std::fs::write(&main, &text).unwrap();
            println!(
                "{name}: scripts.types unchanged {:.2} ms ({:.2}-{:.2}); dry run with types {:.2} ms ({:.2}-{:.2}); {}",
                unchanged.0,
                unchanged.1,
                unchanged.2,
                changed.0,
                changed.1,
                changed.2,
                dir.display()
            );
        }
    });
}

/// A project of `n` components of `fields` f64 fields each and `systems` systems over pairs of
/// them, five systems to a file.
fn generate(dir: &std::path::Path, n: usize, fields: usize, systems: usize) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(
        dir.join("project.toml"),
        "name = \"types-cost\"\nrate = 60\nseed = 1\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("scene.json"),
        "{\"format\": \"pocket-scene\", \"version\": 1, \"entities\": []}\n",
    )
    .unwrap();
    let mut comps = String::from("import { component, field } from \"pocket\";\n");
    for c in 0..n {
        comps.push_str(&format!(
            "export const C{c} = component(\"C{c}\", {{ version: 1, doc: \"Component {c}.\", fields: {{\n"
        ));
        for f in 0..fields {
            comps.push_str(&format!("    f{f}: field.f64({f}, \"Field {f}.\"),\n"));
        }
        comps.push_str("} });\n");
    }
    std::fs::write(dir.join("scripts/components.ts"), comps).unwrap();
    let mut names = Vec::new();
    for file in 0..systems.div_ceil(5) {
        let mut src = String::from("import { system } from \"pocket\";\n");
        for s in file * 5..(file * 5 + 5).min(systems) {
            let (a, b) = (s % n, (s + 1) % n);
            src.push_str(&format!(
                "export const s{s} = system({{ name: \"s{s}\", phase: \"update\", doc: \"System {s}.\",\n    \
                 queries: {{ q: {{ with: [\"C{a}\", \"C{b}\"], fields: [\"C{a}.f0\", \"C{b}.f1\"] }} }},\n    \
                 run(ctx, {{ q }}) {{ const x = q.cols.C{a}.f0; const y = q.cols.C{b}.f1; \
                 for (let r = 0; r < q.len; r++) x[r] += y[r] * ctx.dt; }},\n}});\n"
            ));
            names.push((file, s));
        }
        std::fs::write(dir.join(format!("scripts/rules{file}.ts")), src).unwrap();
    }
    let mut main =
        String::from("import { game } from \"pocket\";\nimport * as c from \"./components\";\n");
    for file in 0..systems.div_ceil(5) {
        main.push_str(&format!("import * as r{file} from \"./rules{file}\";\n"));
    }
    let comps: Vec<String> = (0..n).map(|c| format!("c.C{c}")).collect();
    let sys: Vec<String> = names.iter().map(|(f, s)| format!("r{f}.s{s}")).collect();
    main.push_str(&format!(
        "export default game({{ components: [{}], systems: [{}] }});\n",
        comps.join(", "),
        sys.join(", ")
    ));
    std::fs::write(dir.join("scripts/main.ts"), main).unwrap();
}
