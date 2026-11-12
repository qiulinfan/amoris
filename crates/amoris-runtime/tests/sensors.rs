//! Sensors and collision groups end to end (docs/spec/simulation.md 8.5; script-host.md 7.2;
//! docs/sdk.md): a scene's sensor gate that a scripted walker walks into, a sensor and a filtered
//! ball a script makes, their events read by the script a tick later, `SensorOverlaps` on the
//! walker through `world.get`, and the same hash chains through a recording, a fork and a restore.
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

/// The walker (entity 3) walks forward for 50 ticks into the gate (entity 2) and stops inside it.
/// At tick 1 the script makes a sensor pad and drops a ball through it, and a "ghost" ball in
/// group 2 that meets nothing but group 2, so it falls through the ground, which is in group 1
/// alone (a collider without a filter is in every group, the ghost's too). Every sensor event is
/// echoed as `game.seen {kind, sensor, other}`.
const MAIN: &str = r#"
import { game, system } from "amoris";
import type { Entity } from "amoris";

const WALKER = 3 as unknown as Entity;

export const play = system({
    name: "play", phase: "update", doc: "Walks the walker and drops balls through a sensor.",
    events: ["physics."],
    run(ctx) {
        for (const e of ctx.events) {
            if (e.kind === "physics.sensor_entered" || e.kind === "physics.sensor_left") {
                ctx.emit("game.seen", { kind: e.kind, sensor: e.subject, other: e.data.other });
            }
        }
        ctx.world.set(WALKER, "Walker", { forward: ctx.tick <= 50 ? 1 : 0 });
        if (ctx.tick === 1) {
            ctx.world.spawn({
                Transform: { position: { x: 6, y: 2 } },
                Collider: { kind: "Cuboid", half_extents: { x: 1, y: 0.25, z: 1 } },
                CollisionFilter: { sensor: true },
            });
            ctx.world.spawn({
                Transform: { position: { x: 6, y: 4 } }, Velocity: {},
                RigidBody: { kind: "Dynamic" },
                Collider: { kind: "Ball", radius: 0.25, density: 500 },
            });
            const ghost = ctx.world.spawn({
                Transform: { position: { x: -6, y: 1 } }, Velocity: {},
                RigidBody: { kind: "Dynamic" },
                Collider: { kind: "Ball", radius: 0.25, density: 500 },
            });
            const GHOST = 1 << 1;
            ctx.world.insert(ghost, "CollisionFilter", { groups: GHOST, mask: GHOST });
        }
    },
});

export default game({ components: [], systems: [play] });
"#;

fn scene() -> Value {
    json!([
        {"name": "Ground", "components": {
            "Transform": {"position": [0.0, -0.5, 0.0]},
            "Collider": {"shape": {"Cuboid": {"half_extents": [20.0, 0.5, 20.0]}}},
            "CollisionFilter": {"groups": 1}}},
        {"name": "Gate", "components": {
            "Transform": {"position": [0.0, 1.0, -4.0]},
            "Collider": {"shape": {"Cuboid": {"half_extents": [2.0, 1.0, 0.5]}}},
            "CollisionFilter": {"sensor": true}}},
        {"name": "Walker", "components": {
            "Transform": {"position": [0.0, 1.62, 0.0]},
            "Walker": {"gravity_mps2": 9.81}}},
    ])
}

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-sensors-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        format!("name = 'sensors-{name}'\nrate = 60\nseed = 1\n"),
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), MAIN).unwrap();
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": scene()});
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
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

fn call(g: &mut Game, name: &str, params: Value) -> Value {
    g.apply(&Command::new(Source::Developer(9), 1, name, params))
        .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

/// What a run saw: each `game.seen` as `(tick, kind, sensor, other)`, and each tick's hash.
type Seen = (Vec<(u64, String, u64, u64)>, Vec<String>);

/// Steps `ticks`, returning what they saw.
fn run(g: &mut Game, ticks: u32) -> Seen {
    let (mut seen, mut hashes) = (Vec::new(), Vec::new());
    for _ in 0..ticks {
        let r = g.step().unwrap_or_else(|e| panic!("{e:#?}"));
        assert!(r.errors.is_empty(), "tick {}: {:#?}", r.tick.0, r.errors);
        for e in r.events.iter().filter(|e| e.kind.as_str() == "game.seen") {
            let d = e.data.to_json();
            seen.push((
                r.tick.0,
                d["kind"].as_str().unwrap().to_owned(),
                d["sensor"].as_u64().unwrap(),
                d["other"].as_u64().unwrap(),
            ));
        }
        hashes.push(format!("{:?}", g.world_hash().unwrap()));
    }
    (seen, hashes)
}

#[test]
fn sensors_report_walkers_and_bodies_to_scripts() {
    common::big_stack(|| {
        let root = project("play");
        let mut g = build(&root);
        let (seen, _) = run(&mut g, 120);
        // Entities: ground 1, gate 2, walker 3; the script's pad 4, ball 5, ghost 6.
        let what: Vec<(&str, u64, u64)> = seen
            .iter()
            .map(|(_, k, s, o)| (k.as_str(), *s, *o))
            .collect();
        assert_eq!(
            what,
            [
                ("physics.sensor_entered", 4, 5),
                ("physics.sensor_left", 4, 5),
                ("physics.sensor_entered", 2, 3),
            ],
            "{seen:?}"
        );
        // The walker stopped inside the gate, which the engine keeps on it.
        let w = call(&mut g, "world.get", json!({"entity": "Walker"}));
        assert_eq!(w["components"]["SensorOverlaps"], json!({"sensors": [2]}));
        // The ball rests on the ground; the ghost fell through it.
        let y = |g: &mut Game, id: u64| {
            call(
                g,
                "world.get",
                json!({"entity": id, "fields": ["Transform.position"]}),
            )["fields"]["Transform.position"][1]
                .as_f64()
                .unwrap()
        };
        assert!((y(&mut g, 5) - 0.25).abs() < 0.05, "{}", y(&mut g, 5));
        assert!(y(&mut g, 6) < -2.0, "{}", y(&mut g, 6));
        // An edit that makes the gate solid ends the overlap at the next tick.
        call(
            &mut g,
            "world.edit",
            json!({"ops": [{"set": {"entity": "Gate", "component": "CollisionFilter",
                                    "value": {"sensor": false}}}]}),
        );
        // The step after it finds the walker out of the gate; the script reads it a tick later.
        let (seen, _) = run(&mut g, 2);
        let t = g.tick().0;
        assert_eq!(seen, [(t, "physics.sensor_left".to_owned(), 2, 3)]);
        let w = call(&mut g, "world.get", json!({"entity": "Walker"}));
        assert!(w["components"].get("SensorOverlaps").is_none(), "{w:#}");
        let _ = std::fs::remove_dir_all(&root);
    });
}

#[test]
fn a_recording_a_fork_and_a_restore_agree() {
    common::big_stack(|| {
        let root = project("chain");
        let mut g = build(&root);
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let mut fork = g.fork().unwrap();
        let first = run(&mut g, 90);
        g.finish_recording().unwrap();
        assert_eq!(first.0.len(), 3, "{:?}", first.0);
        let replay = Replay::read(&sink.bytes()).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut fresh = Game::for_replay(&replay).unwrap_or_else(|e| panic!("{e:#?}"));
        let out = verify(&replay, &mut fresh, VerifyOptions::mode(ReplayMode::Verify)).outcome;
        assert_eq!(out, ReplayOutcome::Identical);
        assert_eq!(run(&mut fork, 90), first);

        // A snapshot taken while the walker stands inside the gate keeps its overlap: after the
        // restore the run goes on as it did before, and nobody is told the walker entered again.
        let mut a = build(&root);
        run(&mut a, 60);
        let w = call(&mut a, "world.get", json!({"entity": "Walker"}));
        assert_eq!(w["components"]["SensorOverlaps"], json!({"sensors": [2]}));
        let snap = a.snapshot().unwrap();
        let mut b = a.fork().unwrap();
        let ahead = run(&mut a, 30);
        a.restore(&snap).unwrap();
        assert_eq!(run(&mut a, 30), ahead);
        assert_eq!(run(&mut b, 30), ahead);
        let _ = std::fs::remove_dir_all(&root);
    });
}
