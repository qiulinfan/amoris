//! The player bench (docs/bench/player.md; charter 2, proof target 2, and 3.6: decision models in
//! the tick at hundreds of decisions per second, the LLM never in the tick):
//!
//! - **agent**: the reference skipper (`pocket_runtime::skipper`) plays the sailing course through
//!   the player tools alone, as `Source::Player(0)`: decisions and acts, wall time, decision
//!   cycles per wall second, the cost of a decision cycle outside the ticks (observe and act), and
//!   the token sizes of what it reads (`ceil(bytes / 4)`, README, Token estimates): observations
//!   in text and JSON at the profile's default budget, the game definition, a wait's answer.
//! - **in_tick**: N boats steered every tick toward a point by a decision model in the tick, so
//!   decisions per second = N x ticks per second: native (each boat a seat with the skipper's
//!   perception and a `sail_to` intent, whose Rust executor decides from that perception every
//!   tick) and scripted (a TypeScript system steering every boat, reading the world), beside a
//!   baseline of the same boats with nothing deciding.
//!
//! `cargo run --release -p pocket-runtime --features transpile --example player_bench [out.json]`
//! Timings depend on the machine and its load: the bench doc labels them provisional.

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick
#![allow(clippy::cast_precision_loss)] // counts and microseconds into f64 for reporting

use std::sync::Arc;
use std::time::Instant;

use pocket_link::Source;
use pocket_runtime::skipper::Skipper;
use pocket_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};

fn repo() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn course_project() -> Project {
    Project::load(&repo().join("samples/sailing-course")).unwrap_or_else(|e| panic!("{e:#?}"))
}

fn call(g: &mut Game, source: Source, seq: &mut u64, name: &str, params: Value) -> Value {
    *seq += 1;
    g.apply(&Command::new(source, *seq, name, params))
        .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

fn tokens(bytes: usize) -> u64 {
    bytes.div_ceil(4) as u64
}

fn stats(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return json!(null);
    }
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let round = |x: f64| (x * 10.0).round() / 10.0;
    json!({"n": v.len(), "min": round(v[0]), "median": round(v[v.len() / 2]),
           "max": round(v[v.len() - 1]), "mean": round(mean)})
}

/// The reference skipper plays the course once.
fn agent(setup: &Arc<GameSetup>, seed: u64) -> Value {
    let mut g = Game::new(setup.clone(), seed).unwrap_or_else(|e| panic!("{e:#?}"));
    let p = Source::Player(0);
    let mut seq = 0;
    let mut skipper = Skipper::new();
    let describe = call(&mut g, p, &mut seq, "player.describe", json!({}));
    let describe_bytes = serde_json::to_string(&describe).map_or(0, |s| s.len());
    let intents = call(
        &mut g,
        p,
        &mut seq,
        "player.describe",
        json!({"part": "intents"}),
    );
    let intents_bytes = serde_json::to_string(&intents).map_or(0, |s| s.len());
    let (mut text_tok, mut json_tok, mut big_tok, mut wait_tok) = (vec![], vec![], vec![], vec![]);
    let (mut cycle_us, mut wait_us, mut wait_ticks) = (vec![], vec![], vec![]);
    let (mut decisions, mut acts) = (0u32, 0u32);
    let mut outcome = Value::Null;
    let mut example = Value::Null;
    let start = Instant::now();
    for i in 0..400 {
        let t0 = Instant::now();
        let text = call(
            &mut g,
            p,
            &mut seq,
            "player.observe",
            json!({"projection": "text"}),
        );
        if i == 4 {
            example = text.clone();
        }
        let small = call(&mut g, p, &mut seq, "player.observe", json!({}));
        let obs = call(
            &mut g,
            p,
            &mut seq,
            "player.observe",
            json!({"budget_tokens": 1500}),
        );
        text_tok.push(tokens(text.as_str().map_or(0, str::len)) as f64);
        json_tok.push(small["tokens"].as_f64().unwrap_or(0.0));
        big_tok.push(obs["tokens"].as_f64().unwrap_or(0.0));
        decisions += 1;
        if let Some(act) = skipper.decide(&obs) {
            call(&mut g, p, &mut seq, "player.act", act);
            acts += 1;
        }
        cycle_us.push(t0.elapsed().as_secs_f64() * 1e6);
        let t1 = Instant::now();
        let from = g.tick().0;
        let w = call(&mut g, p, &mut seq, "player.wait", json!({}));
        wait_us.push(t1.elapsed().as_secs_f64() * 1e6);
        wait_ticks.push((g.tick().0 - from) as f64);
        wait_tok.push(tokens(serde_json::to_string(&w).map_or(0, |s| s.len())) as f64);
        if w["stopped"] == json!("done") {
            outcome = w["outcome"].clone();
            break;
        }
    }
    let wall = start.elapsed().as_secs_f64();
    let ticks = g.tick().0;
    let tick_s: f64 = wait_us.iter().sum::<f64>() / 1e6;
    json!({
        "seed": seed,
        "finished": outcome["terminated"] == json!(true),
        "outcome": outcome,
        "decisions": decisions,
        "acts": acts,
        "ticks": ticks,
        "sim_s": ticks as f64 / 60.0,
        "wall_s": (wall * 1000.0).round() / 1000.0,
        "decisions_per_wall_s": (f64::from(decisions) / wall * 10.0).round() / 10.0,
        "ticks_per_wall_s": (ticks as f64 / tick_s).round(),
        "decision_cycle_us": stats(cycle_us),
        "wait_us": stats(wait_us),
        "wait_ticks": stats(wait_ticks),
        "tokens": {
            "observe_text_default_budget": stats(text_tok),
            "observe_json_default_budget": stats(json_tok),
            "observe_json_budget_1500": stats(big_tok),
            "wait_answer_json": stats(wait_tok),
            "describe_definition": tokens(describe_bytes),
            "describe_intents_part": tokens(intents_bytes),
        },
        "example_observation_text": example,
    })
}

/// The scripted decision model: a TypeScript system that steers every boat with a `Pilot` toward
/// its point, trims the sheet to the apparent wind and keeps the sail set, every tick.
const POLICY: &str = r#"import { component, field, system } from "pocket";

export const Pilot = component("Pilot", {
    version: 1, doc: "A point to steer for.",
    fields: { x: field.f64(0, "East, metres."), z: field.f64(0, "South, metres.") },
});

function clamp(x: number, lo: number, hi: number): number {
    return Math.min(hi, Math.max(lo, x));
}

export const pilot = system({
    name: "pilot", phase: "update", doc: "Steers every piloted boat toward its point.",
    queries: { boats: { with: ["Pilot", "Boat", "Transform"], fields: ["Pilot.x", "Pilot.z",
        "Boat.heading_deg", "Boat.awa_deg", "Boat.rudder", "Boat.sheet", "Boat.hoist",
        "Transform.position"] } },
    run(_ctx, { boats }) {
        const p = boats.cols.Pilot;
        const b = boats.cols.Boat;
        const at = boats.cols.Transform.position;
        boats.each((r) => {
            const bearing = Math.atan2(p.x[r] - at.x[r], -(p.z[r] - at.z[r])) * 180 / Math.PI;
            let err = (bearing - b.heading_deg[r]) % 360;
            if (err > 180) err -= 360;
            if (err <= -180) err += 360;
            b.rudder[r] = clamp(err / 30, -1, 1);
            b.sheet[r] = clamp((Math.abs(b.awa_deg[r]) / 2 - 5) / 80, 0, 1);
            b.hoist[r] = 1;
        });
    },
});
"#;

const MAIN: &str = r#"import { game } from "pocket";
import { Cargo, Course, Crew, Mark, Tally } from "./components";
import { muster, rounding, takeAboard } from "./rules";
import { Pilot, pilot } from "./policy";

export default game({
    components: [Crew, Tally, Cargo, Mark, Course, Pilot],
    systems: [muster, takeAboard, rounding, pilot],
});
"#;

/// The course with the pilot policy in its scripts.
fn scripted_setup(p: &Project) -> Arc<GameSetup> {
    let source = p
        .source
        .clone()
        .with("scripts/policy.ts", POLICY)
        .with("scripts/main.ts", MAIN);
    let mut setup = p.setup(false).unwrap_or_else(|e| panic!("{e:#?}"));
    setup.scripts = pocket_runtime::compile(&source, false).unwrap_or_else(|e| panic!("{e:#?}"));
    setup.source = Some(source);
    Arc::new(setup)
}

/// `n` boats in a column 30 m apart north of the course, each steered toward a point 400 m east
/// of where it starts by `variant`: "native" (a seat with the skipper's perception and a
/// `sail_to` intent), "scripted" (the pilot policy) or "baseline" (nothing decides).
fn in_tick(setup: &Arc<GameSetup>, n: usize, variant: &str, ticks: u64) -> Value {
    let mut g = Game::new(setup.clone(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
    let dev = Source::Developer(0);
    let mut seq = 0;
    let mut ops = Vec::new();
    for k in 0..n {
        let z = -150.0 - 30.0 * k as f64;
        let mut components = json!({"Boat": {"hoist": 1, "hoist_now": 1, "sheet": 0.6}});
        match variant {
            "native" => {
                components["Observer"] = json!({"profile": "skipper", "seat": format!("boat_{k}")});
                components["Perceivable"] =
                    json!({"kind": "boat", "detect_m": 1500, "height_m": 6, "priority": 70});
            }
            "scripted" => components["Pilot"] = json!({"x": 400.0, "z": z}),
            _ => {}
        }
        ops.push(json!({"spawn": {"name": format!("Boat{k}"),
            "prefab": {"kind": "sloop", "position": [0, 0, z], "heading_deg": 90},
            "components": components}}));
    }
    call(&mut g, dev, &mut seq, "world.edit", json!({"ops": ops}));
    if variant == "native" {
        for k in 0..n {
            let z = -150.0 - 30.0 * k as f64;
            call(
                &mut g,
                dev,
                &mut seq,
                "player.act",
                json!({"seat": format!("boat_{k}"), "actions": [{"do": "start",
                    "intent": "sail_to", "target": {"x": 400.0, "z": z}}]}),
            );
        }
    }
    g.run(30).unwrap_or_else(|e| panic!("{e:#?}"));
    let mut timer = Timer::default();
    let t0 = Instant::now();
    for _ in 0..ticks {
        g.step_with(&mut timer).unwrap_or_else(|e| panic!("{e:#?}"));
    }
    let wall = t0.elapsed().as_secs_f64();
    let tick_us = wall / ticks as f64 * 1e6;
    let decisions = if variant == "baseline" {
        0.0
    } else {
        n as f64 * ticks as f64 / wall
    };
    let per_tick = |k: &str| (timer.us.get(k).copied().unwrap_or(0.0) / ticks as f64).round();
    json!({"boats": n, "variant": variant, "ticks": ticks,
           "tick_us": tick_us.round(), "decisions_per_s": decisions.round(),
           "intents_us": per_tick("interface.intents"),
           "perception_us": per_tick("interface.perception"),
           "scripts_us": per_tick("script.update")})
}

/// Accumulates the time of every system, by key, outside the tick (script systems run nested in
/// `script.update`, so the open ones are a stack).
#[derive(Default)]
struct Timer {
    open: Vec<Instant>,
    us: std::collections::BTreeMap<String, f64>,
}

impl pocket_sim::StepHooks for Timer {
    fn system(&mut self, key: &pocket_sim::SystemKey, begin: bool) {
        if begin {
            self.open.push(Instant::now());
        } else if let Some(t) = self.open.pop() {
            *self.us.entry(key.as_str().to_owned()).or_default() += t.elapsed().as_secs_f64() * 1e6;
        }
    }
}

fn main() {
    let out = std::env::args().nth(1);
    let run = || {
        let project = course_project();
        let setup = Arc::new(project.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
        let scripted = scripted_setup(&project);
        let agent: Vec<Value> = [1u64, 2, 3].iter().map(|s| agent(&setup, *s)).collect();
        let mut ticking = Vec::new();
        for n in [1usize, 8, 32] {
            for variant in ["baseline", "native", "scripted"] {
                let s = if variant == "scripted" {
                    &scripted
                } else {
                    &setup
                };
                ticking.push(in_tick(s, n, variant, 600));
            }
        }
        json!({"provisional": true, "agent": agent, "in_tick": ticking})
    };
    // Scripts need the script host's stack (script-sandbox.md 4.3).
    let report = std::thread::Builder::new()
        .stack_size(pocket_runtime::GAME_STACK_BYTES)
        .spawn(run)
        .expect("a thread")
        .join()
        .expect("the bench ran");
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    println!("{text}");
    if let Some(path) = out {
        std::fs::write(&path, text + "\n").unwrap_or_else(|e| panic!("{path}: {e}"));
    }
}
