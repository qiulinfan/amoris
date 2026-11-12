//! Joints end to end (docs/spec/simulation.md 8.5; script-host.md 7.2; docs/sdk.md): a scene's
//! hinge and a script's pendulum, a script's refused `with` of none, and the same hash chains
//! through a recording, a fork and a restore, the solver's joints riding in the physics cache.
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

/// At tick 1 a pendulum: a fixed pivot and a bob two metres off it on a spherical joint.
const MAIN: &str = r#"
import { game, system } from "amoris";

export const build = system({
    name: "build", phase: "update", doc: "Hangs a pendulum.", when: "start",
    run(ctx) {
        const pivot = ctx.world.spawn({
            Transform: { position: { x: 4, y: 5 } },
            Collider: { kind: "Ball", radius: 0.1 },
        });
        ctx.world.spawn({
            Transform: { position: { x: 6, y: 5 } }, Velocity: {},
            RigidBody: { kind: "Dynamic" },
            Collider: { kind: "Ball", radius: 0.2, density: 500 },
            Joint: { kind: "Spherical", with: pivot, anchor: { x: -2 } },
        });
    },
});

export default game({ components: [], systems: [build] });
"#;

/// A frame (entity 1) and a door hinged to it about +y, limited to half a radian either way.
fn scene() -> Value {
    json!([
        {"name": "Frame", "components": {
            "Transform": {"position": [0.0, 1.0, 0.0]},
            "Collider": {"shape": {"Ball": {"radius": 0.1}}}}},
        {"name": "Door", "components": {
            "Transform": {"position": [0.5, 1.0, 0.0]}, "Velocity": {"angular": [0.0, 3.0, 0.0]},
            "RigidBody": {"kind": "Dynamic"},
            "Collider": {"shape": {"Cuboid": {"half_extents": [0.5, 1.0, 0.05]}}, "density": 300.0},
            "Joint": {"kind": "Revolute", "with": 1, "anchor": [-0.5, 0.0, 0.0],
                      "axis": [0.0, 1.0, 0.0], "limits": [-0.5, 0.5]}}},
    ])
}

fn project(name: &str, main: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-joints-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        format!("name = 'joints-{name}'\nrate = 60\nseed = 1\n"),
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), main).unwrap();
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

fn position(g: &mut Game, id: u64) -> [f64; 3] {
    let v = call(
        g,
        "world.get",
        json!({"entity": id, "fields": ["Transform.position"]}),
    );
    let p = &v["fields"]["Transform.position"];
    [0, 1, 2].map(|i| p[i].as_f64().unwrap())
}

fn hashes(g: &mut Game, ticks: u32) -> Vec<String> {
    (0..ticks)
        .map(|_| {
            let r = g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            assert!(r.errors.is_empty(), "tick {}: {:#?}", r.tick.0, r.errors);
            g.world_hash().unwrap().to_string()
        })
        .collect()
}

#[test]
fn a_scenes_hinge_and_a_scripts_pendulum_hold() {
    common::big_stack(|| {
        let root = project("hold", MAIN);
        let mut g = build(&root);
        hashes(&mut g, 1);
        // The pendulum (pivot 3, bob 4) keeps its length and swings down through its lowest
        // point.
        let mut lowest = f64::MAX;
        for _ in 0..90 {
            hashes(&mut g, 1);
            let bob = position(&mut g, 4);
            let d = [bob[0] - 4.0, bob[1] - 5.0, bob[2]];
            let len = amoris_sim::math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
            assert!((len - 2.0).abs() < 0.05, "{bob:?}");
            lowest = amoris_sim::math::min(lowest, bob[1]);
        }
        assert!(lowest < 3.2, "{lowest}");
        // The door turned on its hinge and stopped at its limit; the joint reads back.
        let door = call(
            &mut g,
            "world.get",
            json!({"entity": "Door", "fields": ["Transform.rotation", "Joint.kind", "Joint.limits"]}),
        );
        let f = &door["fields"];
        assert_eq!(f["Joint.kind"], json!("Revolute"));
        assert_eq!(f["Joint.limits"], json!([-0.5, 0.5]));
        let q = &f["Transform.rotation"];
        let yaw = 2.0 * amoris_sim::math::atan2(q[1].as_f64().unwrap(), q[3].as_f64().unwrap());
        assert!(yaw > 0.3 && yaw < 0.55, "yaw {yaw}");
        let _ = std::fs::remove_dir_all(&root);
    });
}

#[test]
fn a_script_cannot_tie_a_joint_to_nothing() {
    common::big_stack(|| {
        let main = MAIN.replace("with: pivot,", "with: null,");
        let root = project("none", &main);
        let mut g = build(&root);
        let r = g.step().unwrap();
        let codes: Vec<&str> = r
            .errors
            .iter()
            .map(|p| p.detail["cause"]["code"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(codes, ["script.write_not_held"], "{:#?}", r.errors);
        let _ = std::fs::remove_dir_all(&root);
    });
}

#[test]
fn a_recording_a_fork_and_a_restore_agree() {
    common::big_stack(|| {
        let root = project("chain", MAIN);
        let mut g = build(&root);
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let mut fork = g.fork().unwrap();
        let first = hashes(&mut g, 60);
        // A joint changed between ticks: the bob's rope lengthened by an edit.
        call(
            &mut g,
            "world.edit",
            json!({"ops": [{"set": {"entity": 4, "component": "Joint",
                                    "value": {"anchor": [-2.5, 0.0, 0.0]}}}]}),
        );
        let second = hashes(&mut g, 30);
        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut fresh = Game::for_replay(&replay).unwrap_or_else(|e| panic!("{e:#?}"));
        let out = verify(&replay, &mut fresh, VerifyOptions::mode(ReplayMode::Verify)).outcome;
        assert_eq!(out, ReplayOutcome::Identical);
        assert_eq!(hashes(&mut fork, 60), first);
        // A snapshot mid-swing restored: the run goes on as before.
        let mut a = build(&root);
        hashes(&mut a, 40);
        let snap = a.snapshot().unwrap();
        let mut b = a.fork().unwrap();
        let ahead = hashes(&mut a, 40);
        a.restore(&snap).unwrap();
        assert_eq!(hashes(&mut a, 40), ahead);
        assert_eq!(hashes(&mut b, 40), ahead);
        assert_eq!(second.len(), 30);
        let _ = std::fs::remove_dir_all(&root);
    });
}
