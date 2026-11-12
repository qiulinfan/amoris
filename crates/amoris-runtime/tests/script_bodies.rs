//! Rigid bodies from scripts, end to end (docs/spec/script-host.md 7.2; docs/sdk.md, Rigid
//! bodies): a script spawns a ground and a ball that falls, lands and raises its contact, through
//! a fork and a cold restore; a force, a torque and an impulse written to `ExternalForce` act in
//! the tick they are written and are zero after it; `set` and columns tune the mass, the damping,
//! the gravity scale, the friction and the restitution; a scene's mesh survives a script's writes
//! and what the flat form cannot say is refused at the script's line; a body that breaks the mass
//! rule is reported by `physics.step` until a write mends it; and a write trace of the JSON form's
//! `shape` and `mass` names the script that changed them.
#![cfg(feature = "transpile")]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use amoris_link::Source;
use amoris_runtime::{Command, Game, GameSetup, Project};
use amoris_sim::{EntityId, PlainData, StepReport};
use serde_json::{Value, json};

/// A temporary project of one script and a scene.
fn project(name: &str, main: &str, scene: &Value) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "amoris-script-bodies-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        format!("name = 'bodies-{name}'\nrate = 60\nseed = 1\n"),
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), main).unwrap();
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": scene});
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    root
}

fn load(root: &Path) -> Arc<GameSetup> {
    let p = Project::load(root).unwrap_or_else(|e| panic!("{e:#?}"));
    Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")))
}

fn game(setup: &Arc<GameSetup>) -> Game {
    Game::new(setup.clone(), 1).unwrap_or_else(|e| panic!("{e:#?}"))
}

fn call(g: &mut Game, name: &str, params: Value) -> Value {
    g.apply(&Command::new(Source::Developer(9), 1, name, params))
        .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

/// A field, or a path into one, of an entity named by its name or its id.
fn field(g: &mut Game, entity: impl Into<Value>, path: &str) -> Value {
    let r = call(
        g,
        "world.get",
        json!({"entity": entity.into(), "fields": [path]}),
    );
    r["fields"][path].clone()
}

fn num(g: &mut Game, entity: impl Into<Value>, path: &str) -> f64 {
    let v = field(g, entity, path);
    v.as_f64().unwrap_or_else(|| panic!("{path} is {v}"))
}

/// A whole component as JSON, or null.
fn component(g: &mut Game, entity: impl Into<Value>, name: &str) -> Value {
    let r = call(
        g,
        "world.get",
        json!({"entity": entity.into(), "components": [name]}),
    );
    r["components"][name].clone()
}

/// The id of the entity named `name`.
fn id(g: &Game, name: &str) -> u64 {
    let named = amoris_runtime::named(g.world());
    let found = named.iter().find(|(_, n)| n == name);
    found.unwrap_or_else(|| panic!("no {name}")).0.get()
}

fn step(g: &mut Game) -> StepReport {
    g.step().unwrap_or_else(|e| panic!("{e:#?}"))
}

/// Steps to the end of tick `tick`, refusing any failure on the way; the last tick's report.
fn run_to(g: &mut Game, tick: u64) -> StepReport {
    loop {
        let r = step(g);
        assert!(r.errors.is_empty(), "tick {}: {:#?}", r.tick.0, r.errors);
        if r.tick.0 >= tick {
            return r;
        }
    }
}

/// The contact events of a report between `a` and `b`, in order: true for a begin.
fn contacts(r: &StepReport, a: u64, b: u64) -> Vec<bool> {
    let (subject, other) = (a.min(b), a.max(b));
    let other = PlainData::Number(EntityId::new(other).expect("an entity id").to_f64());
    r.events
        .iter()
        .filter(|e| e.subject.map(|s| s.get()) == Some(subject))
        .filter(|e| e.data.get("other") == Some(&other))
        .filter_map(|e| match e.kind.as_str() {
            "physics.contact_began" => Some(true),
            "physics.contact_ended" => Some(false),
            _ => None,
        })
        .collect()
}

/// Each failure of a report as `(system, cause's code, entity)`.
fn failures(r: &StepReport) -> Vec<(String, String, Option<u64>)> {
    r.errors
        .iter()
        .map(|p| {
            assert_eq!(p.code, "sim.system_failed", "{p:#?}");
            (
                p.detail["system"].as_str().unwrap_or("").to_owned(),
                p.detail["cause"]["code"].as_str().unwrap_or("").to_owned(),
                p.detail["entity"].as_u64(),
            )
        })
        .collect()
}

/// The 1-based line of `text` holding `MARK <name>`.
fn mark(text: &str, name: &str) -> u64 {
    let tag = format!("MARK {name}");
    let at = text.lines().position(|l| l.trim_end().ends_with(&tag));
    u64::try_from(at.unwrap_or_else(|| panic!("no {tag}"))).unwrap() + 1
}

const FALL: &str = r#"
import { component, field, game, system } from "amoris";
import type { Entity } from "amoris";

export const Probe = component("Probe", {
    version: 1, doc: "What the systems made and saw.",
    fields: {
        ground: field.entity("The ground the script spawned."),
        ball: field.entity("The ball the script spawned."),
        contacts: field.u32(0, "Contacts the watcher saw begin."),
        subject: field.entity("The last contact's subject."),
        other: field.entity("The last contact's other entity."),
    },
});

export const make = system({
    name: "make", phase: "update", doc: "Spawns a fixed ground and a ball above it.",
    when: "start",
    run(ctx) {
        const ground = ctx.world.spawn({
            Transform: { position: { y: -0.5 } },
            Collider: { kind: "Cuboid", half_extents: { x: 10, y: 0.5, z: 10 } },
        });
        const ball = ctx.world.spawn({
            Transform: { position: { y: 2 } },
            Velocity: {},
            RigidBody: { kind: "Dynamic" },
            Collider: { kind: "Ball", radius: 0.2, density: 500 },
        });
        ctx.world.set(ctx.single("Probe"), "Probe", { ground, ball });
    },
});

export const watch = system({
    name: "watch", phase: "update", doc: "Counts the contacts that began.",
    events: ["physics.contact_began"],
    run(ctx) {
        const probe = ctx.single("Probe");
        for (const ev of ctx.events) {
            const seen = ctx.world.get(probe, "Probe")!;
            const other = (ev.data as { other: Entity }).other;
            ctx.world.set(probe, "Probe", { contacts: seen.contacts + 1, subject: ev.subject, other });
        }
    },
});

export default game({ components: [Probe], systems: [make, watch] });
"#;

#[test]
fn a_script_makes_a_ball_that_falls_lands_and_raises_its_contact() {
    common::big_stack(|| {
        let scene = json!([{"name": "Probe", "components": {"Probe": {}}}]);
        let root = project("fall", FALL, &scene);
        let setup = load(&root);
        let mut g = game(&setup);

        // The declarations name the three components and the collider's kinds.
        let types = call(&mut g, "scripts.types", json!({"text": true}));
        let names = |k: &str| types["components"][k].as_array().unwrap().clone();
        for name in ["RigidBody", "Collider", "ExternalForce"] {
            assert!(names("engine").contains(&json!(name)), "{name}: {types}");
            assert!(!names("unavailable").contains(&json!(name)), "{name}");
        }
        let dts = types["text"]["components.d.ts"].as_str().unwrap();
        assert!(
            dts.contains("readonly kind: \"Dynamic\" | \"Fixed\";"),
            "{dts}"
        );
        assert!(
            dts.contains(
                "readonly kind: \"Ball\" | \"Cuboid\" | \"Capsule\" | \"ConvexHull\" | \
                 \"TriMesh\" | \"Compound\" | \"HeightField\";"
            ),
            "{dts}"
        );

        // Both bodies are built in the tick that spawned them: the ball is falling after it.
        let first = step(&mut g);
        assert!(first.errors.is_empty(), "{:#?}", first.errors);
        let ground = field(&mut g, "Probe", "Probe.ground").as_u64().unwrap();
        let ball = field(&mut g, "Probe", "Probe.ball").as_u64().unwrap();
        assert!(num(&mut g, ball, "Velocity.linear.1") < 0.0);
        assert_eq!(
            component(&mut g, ball, "Collider"),
            json!({"shape": {"Ball": {"radius": 0.2}}, "density": 500.0, "friction": 0.5,
                   "restitution": 0.0})
        );
        assert_eq!(
            component(&mut g, ground, "Collider")["shape"],
            json!({"Cuboid": {"half_extents": [10.0, 0.5, 10.0]}})
        );
        assert_eq!(
            component(&mut g, ground, "Collider")["density"],
            Value::Null
        );
        assert_eq!(component(&mut g, ball, "RigidBody")["kind"], "Dynamic");
        assert_eq!(component(&mut g, ground, "RigidBody"), Value::Null);

        // Mid-fall: a fork and a snapshot, which a cold game restores below.
        run_to(&mut g, 20);
        assert!(num(&mut g, ball, "Transform.position.1") > 1.0);
        let snap = g.snapshot().unwrap();
        let mut fork = g.fork().unwrap();
        let mut hashes = Vec::new();
        let mut landed = Vec::new();
        let mut physics_events = 0;
        for _ in 20..180 {
            let r = step(&mut g);
            assert!(r.errors.is_empty(), "tick {}: {:#?}", r.tick.0, r.errors);
            let f = step(&mut fork);
            assert!(f.errors.is_empty(), "{:#?}", f.errors);
            hashes.push(g.world_hash().unwrap());
            assert_eq!(fork.world_hash().unwrap(), *hashes.last().unwrap());
            landed.extend(contacts(&r, ground, ball));
            let physics = |e: &&amoris_sim::Event| e.kind.as_str().starts_with("physics.");
            physics_events += r.events.iter().filter(physics).count();
        }
        // It landed once, the ground (the lower id) the contact's subject, and rests there.
        assert_eq!(landed, [true]);
        assert_eq!(physics_events, 1);
        let y = num(&mut g, ball, "Transform.position.1");
        assert!((y - 0.2).abs() < 0.02, "the ball rests at {y}");
        assert!(num(&mut g, ball, "Velocity.linear.1").abs() < 0.05);
        // The script read the contact a tick later: the ground (the lower id) is its subject.
        assert_eq!(field(&mut g, "Probe", "Probe.contacts"), 1);
        assert_eq!(field(&mut g, "Probe", "Probe.subject"), ground);
        assert_eq!(field(&mut g, "Probe", "Probe.other"), ball);
        drop((g, fork));

        let mut cold = game(&setup);
        cold.restore(&snap).unwrap();
        assert_eq!(cold.world_hash().unwrap(), snap.world_hash());
        for (k, h) in hashes.iter().enumerate() {
            cold.step().unwrap();
            assert_eq!(cold.world_hash().unwrap(), *h, "tick 20 + {}", k + 1);
        }
        std::fs::remove_dir_all(root).unwrap();
    });
}

const PUSH: &str = r#"
import { game, system } from "amoris";

export const push = system({
    name: "push", phase: "update", doc: "Pushes the puck: a force, a torque, then an impulse.",
    queries: { pucks: { with: ["ExternalForce", "RigidBody"], fields: ["ExternalForce.force", "ExternalForce.torque"] } },
    run(ctx, { pucks }) {
        const { force, torque } = pucks.cols.ExternalForce;
        for (let r = 0; r < pucks.len; r++) {
            if (ctx.tick === 5) {
                ctx.world.set(pucks.id(r), "ExternalForce", { force: { x: 12 } });
                torque.y[r] += 0.3;
            }
            if (ctx.tick === 10) force.x[r] += 3 / ctx.dt;
        }
    },
});

export default game({ systems: [push] });
"#;

#[test]
fn a_force_a_torque_and_an_impulse_act_in_the_tick_they_are_written() {
    common::big_stack(|| {
        // 2 kg and 0.4 kg m^2, stated; no gravity.
        let scene = json!([{"name": "Puck", "components": {
            "Transform": {"position": [0.0, 5.0, 0.0]},
            "Velocity": {},
            "ExternalForce": {},
            "RigidBody": {"kind": "Dynamic", "gravity_scale": 0.0,
                          "mass": {"mass": 2.0, "center": [0.0, 0.0, 0.0],
                                   "inertia": [0.4, 0.4, 0.4]}},
            "Collider": {"shape": {"Ball": {"radius": 0.5}}}}}]);
        let root = project("push", PUSH, &scene);
        let mut g = game(&load(&root));
        run_to(&mut g, 4);
        assert_eq!(num(&mut g, "Puck", "Velocity.linear.0"), 0.0);
        // Tick 5: 12 N for one step of 1/60 s on 2 kg, and 0.3 N m on 0.4 kg m^2.
        run_to(&mut g, 5);
        let v = num(&mut g, "Puck", "Velocity.linear.0");
        assert!((v - 0.1).abs() < 1e-6, "{v}");
        let w = num(&mut g, "Puck", "Velocity.angular.1");
        assert!((w - 0.0125).abs() < 1e-6, "{w}");
        assert_eq!(
            component(&mut g, "Puck", "ExternalForce"),
            json!({"force": [0.0, 0.0, 0.0], "torque": [0.0, 0.0, 0.0]}),
            "zero at the boundary"
        );
        // The force acted for that step alone.
        run_to(&mut g, 9);
        assert_eq!(num(&mut g, "Puck", "Velocity.linear.0"), v);
        let spin = num(&mut g, "Puck", "Velocity.angular.1");
        assert!((spin - w).abs() < 1e-6, "{spin} after {w}");
        // Tick 10: an impulse of 3 N s is 3 / dt newtons for one tick: 1.5 m/s on 2 kg.
        run_to(&mut g, 11);
        let after = num(&mut g, "Puck", "Velocity.linear.0");
        assert!((after - v - 1.5).abs() < 1e-5, "{after}");
        std::fs::remove_dir_all(root).unwrap();
    });
}

const TUNE: &str = r#"
import { component, field, game, system } from "amoris";

export const Tag = component("Tag", {
    version: 1, doc: "Which body of the test this is.",
    fields: { n: field.u32(0, "Its number.") },
});

const LIGHT = 1;
const HEAVY = 2;
const STATED = 3;
const DAMPED = 5;
const HOVER = 6;
const GRIP = 9;
const BOUNCY = 11;

export const tune = system({
    name: "tune", phase: "update", doc: "Tunes the bodies, each change in a tick of its own.",
    queries: {
        bodies: {
            with: ["Tag", "RigidBody", "Collider", "ExternalForce"],
            fields: ["Tag.n", "RigidBody.linear_damping", "Collider.friction", "ExternalForce.force"],
        },
    },
    run(ctx, { bodies }) {
        const n = bodies.cols.Tag.n;
        const damping = bodies.cols.RigidBody.linear_damping;
        const friction = bodies.cols.Collider.friction;
        const force = bodies.cols.ExternalForce.force;
        for (let r = 0; r < bodies.len; r++) {
            const e = bodies.id(r);
            if (ctx.tick === 1) {
                if (n[r] === HOVER) ctx.world.set(e, "RigidBody", { gravity_scale: 0 });
                if (n[r] === BOUNCY) ctx.world.set(e, "Collider", { restitution: 1 });
            }
            if (ctx.tick === 2) {
                if (n[r] === HEAVY) ctx.world.set(e, "Collider", { density: 2000 });
                if (n[r] === STATED) {
                    ctx.world.set(e, "Collider", { density: 0 });
                    ctx.world.set(e, "RigidBody", { mass_kg: 4, inertia: { x: 1, y: 1, z: 1 } });
                }
                if (n[r] === DAMPED) damping[r] = 2;
            }
            if (ctx.tick === 4) {
                if (n[r] === LIGHT || n[r] === HEAVY) force.x[r] += 600 / ctx.dt;
                if (n[r] === STATED) force.x[r] += 2 / ctx.dt;
            }
            if (ctx.tick === 10 && n[r] === GRIP) friction[r] = 2.5;
            if (ctx.tick === 31 && n[r] === HOVER) ctx.world.set(e, "RigidBody", { gravity_scale: 1 });
        }
    },
});

export default game({ components: [Tag], systems: [tune] });
"#;

/// A dynamic body of the tuning scene: its tag, where it is, how it moves, its gravity scale and
/// its collider.
fn tuned(name: &str, n: u32, at: [f64; 3], vx: f64, gravity: f64, shape: Value) -> Value {
    json!({"name": name, "components": {
        "Tag": {"n": n},
        "Transform": {"position": at},
        "Velocity": {"linear": [vx, 0.0, 0.0]},
        "ExternalForce": {},
        "RigidBody": {"kind": "Dynamic", "gravity_scale": gravity},
        "Collider": {"shape": shape, "density": 1000.0}}})
}

/// Pairs of bodies that differ only in the field a script tunes, compared with margins: the
/// figures a pair gets depend on the solver's combine rules, which this test does not pin.
#[test]
fn set_and_columns_tune_mass_damping_gravity_friction_and_restitution() {
    common::big_stack(|| {
        let ball = |r: f64| json!({"Ball": {"radius": r}});
        let slab = json!({"Cuboid": {"half_extents": [0.5, 0.1, 0.5]}});
        let scene = json!([
            {"name": "Ground", "components": {
                "Transform": {"position": [0.0, -0.5, 0.0]},
                "Collider": {"shape": {"Cuboid": {"half_extents": [60.0, 0.5, 60.0]}}}}},
            // Density and stated mass: three balls adrift, pushed by an impulse at tick 4.
            tuned("Light", 1, [0.0, 10.0, 0.0], 0.0, 0.0, ball(0.5)),
            tuned("Heavy", 2, [0.0, 10.0, 5.0], 0.0, 0.0, ball(0.5)),
            tuned("Stated", 3, [0.0, 10.0, 10.0], 0.0, 0.0, ball(0.5)),
            // Damping: two pucks adrift at 2 m/s.
            tuned("Free", 4, [0.0, 20.0, 0.0], 2.0, 0.0, ball(0.5)),
            tuned("Damped", 5, [0.0, 20.0, 5.0], 2.0, 0.0, ball(0.5)),
            // Gravity scale: two balls high above the ground.
            tuned("Hover", 6, [20.0, 30.0, 0.0], 0.0, 1.0, ball(0.5)),
            tuned("Faller", 7, [20.0, 30.0, 5.0], 0.0, 1.0, ball(0.5)),
            // Friction: two slabs sliding on the ground at 4 m/s.
            tuned("Slide", 8, [0.0, 0.1, 20.0], 4.0, 1.0, slab.clone()),
            tuned("Grip", 9, [0.0, 0.1, 25.0], 4.0, 1.0, slab),
            // Restitution: two balls dropped on the ground.
            tuned("Dead", 10, [-10.0, 1.5, 0.0], 0.0, 1.0, ball(0.25)),
            tuned("Bouncy", 11, [-15.0, 1.5, 0.0], 0.0, 1.0, ball(0.25)),
        ]);
        let root = project("tune", TUNE, &scene);
        let mut g = game(&load(&root));
        let (ground, grip) = (id(&g, "Ground"), id(&g, "Grip"));

        // Tick 2: the density doubles and a stated mass replaces another's; tick 4: the impulses.
        run_to(&mut g, 4);
        assert_eq!(field(&mut g, "Heavy", "Collider.density"), 2000.0);
        assert_eq!(field(&mut g, "Stated", "Collider.density"), Value::Null);
        assert_eq!(
            field(&mut g, "Stated", "RigidBody.mass"),
            json!({"mass": 4.0, "center": [0.0, 0.0, 0.0], "inertia": [1.0, 1.0, 1.0]})
        );
        let light = num(&mut g, "Light", "Velocity.linear.0");
        let heavy = num(&mut g, "Heavy", "Velocity.linear.0");
        assert!(light > 1.0, "600 N s on about 524 kg: {light}");
        assert!(
            (heavy / light - 0.5).abs() < 0.01,
            "{heavy} against {light}"
        );
        let stated = num(&mut g, "Stated", "Velocity.linear.0");
        assert!((stated - 0.5).abs() < 1e-4, "2 N s on 4 kg: {stated}");

        // Tick 10: the grip's friction changes, which rebuilds its body: its contact with the
        // ground ends and begins again in that tick.
        let r = run_to(&mut g, 10);
        assert_eq!(contacts(&r, ground, grip), [false, true], "{:?}", r.events);
        assert_eq!(field(&mut g, "Grip", "Collider.friction"), 2.5);

        // Gravity scale 0 from the first tick: the hover has not moved, its twin has fallen.
        run_to(&mut g, 30);
        assert_eq!(num(&mut g, "Hover", "Transform.position.1"), 30.0);
        assert!(num(&mut g, "Faller", "Transform.position.1") < 29.0);

        // The dropped balls land near tick 31: the highest each is seen from tick 36 to 75.
        let mut top = [0.0f64; 2];
        let mut slid = None;
        for tick in 31..=75 {
            run_to(&mut g, tick);
            if tick == 40 {
                // The slabs: the one a column gave more friction has stopped, well short of the
                // other, which still slides.
                let x = |g: &mut Game, name: &str| num(g, name, "Transform.position.0");
                let v = |g: &mut Game, name: &str| num(g, name, "Velocity.linear.0");
                slid = Some((x(&mut g, "Slide"), x(&mut g, "Grip")));
                assert!(v(&mut g, "Slide") > 0.3, "{}", v(&mut g, "Slide"));
                assert!(v(&mut g, "Grip").abs() < 0.05, "{}", v(&mut g, "Grip"));
            }
            if tick >= 36 {
                for (i, name) in ["Dead", "Bouncy"].into_iter().enumerate() {
                    let y = num(&mut g, name, "Transform.position.1");
                    if y > top[i] {
                        top[i] = y;
                    }
                }
            }
        }
        let (slide, grip_x) = slid.unwrap();
        assert!(slide - grip_x > 0.3, "{slide} against {grip_x}");
        assert!(top[0] < 0.3, "the dead ball stayed down: {}", top[0]);
        assert!(top[1] > 0.4, "the bouncy ball rebounded: {}", top[1]);

        // Damping through a column at tick 2: one puck has slowed, its twin has not.
        let free = num(&mut g, "Free", "Velocity.linear.0");
        let damped = num(&mut g, "Damped", "Velocity.linear.0");
        assert!(free > 1.99, "{free}");
        assert!(damped > 0.0 && damped < 0.5 * free, "{damped}");
        // Gravity scale 1 again at tick 31: the hover falls.
        assert!(num(&mut g, "Hover", "Transform.position.1") < 29.5);
        std::fs::remove_dir_all(root).unwrap();
    });
}

const AUTHORED: &str = r#"
import { component, field, game, system } from "amoris";

export const Ground = component("Ground", { version: 1, doc: "Marks the mesh.", fields: {} });
export const Rock = component("Rock", { version: 1, doc: "Marks the hull.", fields: {} });
export const Ball = component("Ball", { version: 1, doc: "Marks the ball.", fields: {} });
export const Bare = component("Bare", { version: 1, doc: "Marks the entity without a collider.", fields: {} });
export const Probe = component("Probe", {
    version: 1, doc: "What a script read of the scene's colliders.",
    fields: {
        mesh: field.bool(false, "The ground's kind read TriMesh."),
        hull: field.bool(false, "The rock's kind read ConvexHull."),
        sizes: field.f64(-1, "The sum of the sizes the two read."),
        friction: field.f64(-1, "The ground's friction."),
    },
});

export const look = system({
    name: "look", phase: "update", doc: "Reads the scene's mesh and hull.",
    run(ctx) {
        if (ctx.tick !== 5) return;
        const g = ctx.world.get(ctx.single("Ground"), "Collider")!;
        const r = ctx.world.get(ctx.single("Rock"), "Collider")!;
        const size = (c: typeof g) => c.radius + c.half_height + c.half_extents.x + c.half_extents.y + c.half_extents.z;
        ctx.world.set(ctx.single("Probe"), "Probe", {
            mesh: g.kind === "TriMesh", hull: r.kind === "ConvexHull",
            sizes: size(g) + size(r), friction: g.friction,
        });
    },
});

export const same = system({
    name: "same", phase: "update", doc: "Writes the ground's friction as it is.",
    run(ctx) {
        if (ctx.tick === 10) ctx.world.set(ctx.single("Ground"), "Collider", { friction: 0.5 });
    },
});

export const grip = system({
    name: "grip", phase: "update", doc: "Changes the ground's friction.",
    run(ctx) {
        if (ctx.tick === 20) ctx.world.set(ctx.single("Ground"), "Collider", { friction: 0.9 });
    },
});

export const resize = system({
    name: "resize", phase: "update", doc: "Asks a hull for a radius, after a write that would hold.",
    run(ctx) {
        if (ctx.tick !== 30) return;
        const rock = ctx.single("Rock");
        ctx.world.set(rock, "RigidBody", { linear_damping: 0.3 });
        ctx.world.set(rock, "Collider", { radius: 1 }); // MARK resize
    },
});

export const mesh = system({
    name: "mesh", phase: "update", doc: "Asks for a mesh.",
    run(ctx) {
        if (ctx.tick === 40) ctx.world.insert(ctx.single("Bare"), "Collider", { kind: "TriMesh" }); // MARK mesh
    },
});

export const column = system({
    name: "column", phase: "update", doc: "Writes a cuboid's size on a ball's row.", // MARK column
    queries: { balls: { with: ["Ball", "Collider"] } },
    run(ctx, { balls }) {
        if (ctx.tick === 50) balls.cols.Collider.half_extents.x[0] = 1;
    },
});

export default game({
    components: [Ground, Rock, Ball, Bare, Probe],
    systems: [look, same, grip, resize, mesh, column],
});
"#;

#[test]
fn authored_shapes_survive_and_what_the_flat_form_cannot_say_is_refused() {
    common::big_stack(|| {
        // A quad of two triangles facing up.
        let quad = json!({"TriMesh": {
            "vertices": [[-10.0, 0.0, -10.0], [10.0, 0.0, -10.0], [10.0, 0.0, 10.0],
                         [-10.0, 0.0, 10.0]],
            "indices": [[0, 2, 1], [0, 3, 2]]}});
        let hull = json!({"ConvexHull": {"points": [
            [-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.0, 0.5, -0.5], [0.0, 0.0, 0.5]]}});
        let scene = json!([
            {"name": "Ground", "components": {
                "Ground": {},
                "Transform": {"position": [0.0, 0.0, 0.0]},
                "RigidBody": {"kind": "Fixed"},
                "Collider": {"shape": quad}}},
            {"name": "Rock", "components": {
                "Rock": {},
                "Transform": {"position": [5.0, 3.0, 5.0]},
                "Velocity": {},
                "RigidBody": {"kind": "Dynamic", "gravity_scale": 0.0,
                              "mass": {"mass": 80.0, "center": [0.0, 0.0, 0.0],
                                       "inertia": [10.0, 10.0, 10.0]}},
                "Collider": {"shape": hull}}},
            {"name": "Ball", "components": {
                "Ball": {},
                "Transform": {"position": [0.0, 0.25, 0.0]},
                "Velocity": {},
                "RigidBody": {"kind": "Dynamic"},
                "Collider": {"shape": {"Ball": {"radius": 0.25}}, "density": 1000.0}}},
            {"name": "Bare", "components": {
                "Bare": {}, "Transform": {"position": [9.0, 9.0, 9.0]}}},
            {"name": "Probe", "components": {"Probe": {}}},
        ]);
        let root = project("authored", AUTHORED, &scene);
        let mut g = game(&load(&root));
        let (ground, rock, ball, bare) = (
            id(&g, "Ground"),
            id(&g, "Rock"),
            id(&g, "Ball"),
            id(&g, "Bare"),
        );
        let collider = |friction: f64| json!({"shape": quad, "density": null, "friction": friction, "restitution": 0.0});

        // A script reads the mesh and the hull as their kinds, with sizes of 0.
        run_to(&mut g, 5);
        assert_eq!(field(&mut g, "Probe", "Probe.mesh"), true);
        assert_eq!(field(&mut g, "Probe", "Probe.hull"), true);
        assert_eq!(field(&mut g, "Probe", "Probe.sizes"), 0.0);
        assert_eq!(field(&mut g, "Probe", "Probe.friction"), 0.5);

        // A write of the friction it has: the collider is the scene's and the body was kept.
        run_to(&mut g, 9);
        let r = run_to(&mut g, 10);
        assert_eq!(component(&mut g, "Ground", "Collider"), collider(0.5));
        assert_eq!(
            contacts(&r, ground, ball),
            [] as [bool; 0],
            "{:?}",
            r.events
        );

        // A new friction: the mesh is still the scene's, and the body was built again once.
        run_to(&mut g, 19);
        let r = run_to(&mut g, 20);
        assert_eq!(component(&mut g, "Ground", "Collider"), collider(0.9));
        assert_eq!(contacts(&r, ground, ball), [false, true], "{:?}", r.events);

        // A radius asked of the hull: refused at its line, and the call's first write with it.
        run_to(&mut g, 29);
        let r = step(&mut g);
        assert_eq!(
            failures(&r),
            [(
                "script:resize".to_owned(),
                "script.write_not_held".to_owned(),
                Some(rock)
            )]
        );
        let cause = &r.errors[0].detail["cause"];
        assert_eq!(cause["detail"]["component"], "Collider", "{cause:#}");
        assert_eq!(cause["detail"]["field"], "radius", "{cause:#}");
        assert_eq!(cause["detail"]["phase"], "run", "{cause:#}");
        assert_eq!(
            cause["detail"]["location"]["file"], "scripts/main.ts",
            "{cause:#}"
        );
        assert_eq!(
            cause["detail"]["location"]["line"],
            mark(AUTHORED, "resize"),
            "{cause:#}"
        );
        assert_eq!(field(&mut g, "Rock", "RigidBody.linear_damping"), 0.0);
        assert_eq!(component(&mut g, "Rock", "Collider")["shape"], hull);

        // A mesh asked for.
        run_to(&mut g, 39);
        let r = step(&mut g);
        assert_eq!(
            failures(&r),
            [(
                "script:mesh".to_owned(),
                "script.write_not_held".to_owned(),
                Some(bare)
            )]
        );
        let cause = &r.errors[0].detail["cause"];
        assert_eq!(cause["detail"]["field"], "kind", "{cause:#}");
        assert_eq!(
            cause["detail"]["location"]["line"],
            mark(AUTHORED, "mesh"),
            "{cause:#}"
        );
        assert_eq!(component(&mut g, "Bare", "Collider"), Value::Null);

        // A cuboid's size written in a ball's row: refused at the write-back, for that row.
        run_to(&mut g, 49);
        let r = step(&mut g);
        assert_eq!(
            failures(&r),
            [(
                "script:column".to_owned(),
                "script.write_not_held".to_owned(),
                Some(ball)
            )]
        );
        let cause = &r.errors[0].detail["cause"];
        assert_eq!(cause["detail"]["field"], "half_extents", "{cause:#}");
        assert_eq!(cause["detail"]["phase"], "write_back", "{cause:#}");

        // Through all of it the mesh held the ball.
        run_to(&mut g, 60);
        let y = num(&mut g, "Ball", "Transform.position.1");
        assert!((y - 0.25).abs() < 0.02, "the ball rests at {y}");
        assert_eq!(
            component(&mut g, "Ball", "Collider")["shape"],
            json!({"Ball": {"radius": 0.25}})
        );
        std::fs::remove_dir_all(root).unwrap();
    });
}

const MASSLESS: &str = r#"
import { component, game, system } from "amoris";

export const Loose = component("Loose", { version: 1, doc: "The ball without a mass.", fields: {} });
export const Flat = component("Flat", { version: 1, doc: "The cuboid without a size.", fields: {} });

export const make = system({
    name: "make", phase: "update", doc: "Spawns a dynamic ball without a mass and a cuboid without a size.",
    when: "start",
    run(ctx) {
        ctx.world.spawn({
            Loose: {},
            Transform: { position: { y: 5 } },
            Velocity: {},
            RigidBody: {},
            Collider: { kind: "Ball", radius: 0.2 },
        });
        ctx.world.spawn({ Flat: {}, Transform: { position: { x: 3, y: 5 } }, Collider: { kind: "Cuboid" } });
    },
});

export const mend = system({
    name: "mend", phase: "update", doc: "Gives the ball its density and the cuboid its size.",
    run(ctx) {
        if (ctx.tick !== 10) return;
        ctx.world.set(ctx.single("Loose"), "Collider", { density: 500 });
        ctx.world.set(ctx.single("Flat"), "Collider", { half_extents: { x: 1, y: 1, z: 1 } });
    },
});

export default game({ components: [Loose, Flat], systems: [make, mend] });
"#;

/// numeric.md 11.11: the mass rule and the sizes are `physics.step`'s to judge, for a script's
/// bodies as for a scene's. The script sees no throw and the game goes on; the step's report
/// names the entity and the reason every tick, and builds nothing, until a write mends it.
#[test]
fn a_body_without_a_mass_is_reported_by_the_step_until_a_write_mends_it() {
    common::big_stack(|| {
        let root = project("massless", MASSLESS, &json!([]));
        let mut g = game(&load(&root));
        let mut ids = None;
        for tick in 1..10 {
            let r = step(&mut g);
            let failed = failures(&r);
            let (loose, flat) = *ids.get_or_insert_with(|| {
                (
                    failed[0].2.expect("an entity"),
                    failed[1].2.expect("an entity"),
                )
            });
            assert_eq!(
                failed,
                [
                    (
                        "physics.step".to_owned(),
                        "sim.body_invalid".to_owned(),
                        Some(loose)
                    ),
                    (
                        "physics.step".to_owned(),
                        "sim.collider_invalid".to_owned(),
                        Some(flat)
                    ),
                ],
                "tick {tick}"
            );
            let reason = r.errors[0].detail["cause"]["detail"]["reason"]
                .as_str()
                .unwrap();
            assert!(reason.contains("a dynamic body needs its mass"), "{reason}");
            // Neither was built: the ball does not fall.
            assert_eq!(num(&mut g, loose, "Transform.position.1"), 5.0);
        }
        let (loose, flat) = ids.unwrap();
        assert_eq!(component(&mut g, loose, "Loose"), json!({}));
        assert_eq!(component(&mut g, flat, "Flat"), json!({}));
        // Tick 10 mends both: no report from then on, and the ball falls.
        run_to(&mut g, 30);
        assert!(num(&mut g, loose, "Transform.position.1") < 4.5);
        assert_eq!(
            num(&mut g, flat, "Transform.position.1"),
            5.0,
            "a fixed body"
        );
        std::fs::remove_dir_all(root).unwrap();
    });
}

const RESIZE: &str = r#"
import { game, system } from "amoris";

export const resize = system({
    name: "resize", phase: "update", doc: "Resizes the ball at tick 3 and restates the puck's inertia at tick 5.",
    queries: { bodies: { with: ["RigidBody", "Collider"], fields: [] } },
    run(ctx, { bodies }) {
        if (ctx.tick === 3) ctx.world.set(bodies.id(0), "Collider", { radius: 0.3 });
        if (ctx.tick === 5) ctx.world.set(bodies.id(1), "RigidBody", { inertia: { x: 2 } });
    },
});

export default game({ systems: [resize] });
"#;

/// debugger.md 13: the write trace reads a JSON path's change from the script field of the same
/// name. `Collider.shape` and `RigidBody.mass` have none, so a change of a radius or of an inertia
/// alone is not missed.
#[test]
fn a_trace_of_the_json_shape_names_the_script_that_resized_it() {
    common::big_stack(|| {
        let body = |name: &str, x: f64, rigid: Value, collider: Value| {
            json!({"name": name, "components": {
                "Transform": {"position": [x, 5.0, 0.0]}, "Velocity": {},
                "RigidBody": rigid, "Collider": collider}})
        };
        let ball = json!({"Ball": {"radius": 0.25}});
        let stated = json!({"mass": 2.0, "center": [0.0, 0.0, 0.0], "inertia": [1.0, 1.0, 1.0]});
        let scene = json!([
            body(
                "Ball",
                0.0,
                json!({"gravity_scale": 0.0}),
                json!({"shape": ball, "density": 1000.0})
            ),
            body(
                "Puck",
                5.0,
                json!({"gravity_scale": 0.0, "mass": stated}),
                json!({"shape": ball})
            ),
        ]);
        let root = project("trace", RESIZE, &scene);
        let mut g = game(&load(&root));
        let mut seq = 0;
        let mut step_traced = |g: &mut Game, ticks: u64, trace: Value| {
            seq += 1;
            let params = if trace.is_null() {
                json!({"ticks": ticks})
            } else {
                json!({"ticks": ticks, "trace": trace})
            };
            g.apply(&Command::new(
                Source::Developer(0),
                seq,
                "time.step",
                params,
            ))
            .unwrap_or_else(|e| panic!("{e:#?}"))
        };
        step_traced(&mut g, 2, Value::Null);
        let trace = json!({"entity": "Ball", "component": "Collider", "field": "shape"});
        let r = step_traced(&mut g, 1, trace);
        let rows = r["writes"]["rows"].as_array().cloned().unwrap_or_default();
        assert_eq!(rows.len(), 1, "{r:#}");
        assert_eq!(rows[0]["tick"], 3, "{r:#}");
        assert_eq!(rows[0]["by"], "script:resize", "{r:#}");
        assert_eq!(
            rows[0]["before"],
            json!({"Ball": {"radius": 0.25}}),
            "{r:#}"
        );
        assert_eq!(rows[0]["after"], json!({"Ball": {"radius": 0.3}}), "{r:#}");

        step_traced(&mut g, 1, Value::Null);
        let trace = json!({"entity": "Puck", "component": "RigidBody", "field": "mass"});
        let r = step_traced(&mut g, 1, trace);
        let rows = r["writes"]["rows"].as_array().cloned().unwrap_or_default();
        assert_eq!(rows.len(), 1, "{r:#}");
        assert_eq!(rows[0]["tick"], 5, "{r:#}");
        assert_eq!(rows[0]["by"], "script:resize", "{r:#}");
        assert_eq!(rows[0]["after"]["inertia"], json!([2.0, 1.0, 1.0]), "{r:#}");
        std::fs::remove_dir_all(root).unwrap();
    });
}
