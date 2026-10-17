//! QuickJS-ng microloops in the script host (docs/bench/script.md, section 7): the interpreter's
//! cost per iteration of a few loop shapes, and what one call of a `Math` function adds, measured in
//! the hardened context scripts run in (the lockdown's `Math`, the real host, `Sim::step`).
//!
//! Each probe is a project of one system whose `run` loops `N` times over one statement; a round
//! builds a fresh world per probe (in an order that rotates by round, from `--offset`), steps it
//! `--warmup` ticks, then times `--ticks` ticks one by one (`Sim::step`). A probe's cost per
//! iteration is its tick median over `N`; a `Math` call's cost is its probe's less the `loop`
//! probe's (`s += i * 0.001` against `s += Math.sin(i * 0.001)`: the lookup of `Math.sin` and the
//! call). The fixed cost of a tick (about 10 us) is under 0.3 % of the shortest probe's.
//!
//! ```text
//! cargo run --release -p amoris-script --features transpile --example qjs_micro -- \
//!     [--rounds 3] [--offset 0] [--warmup 3] [--ticks 15] [--probes loop,sin,...] [--json out.json]
//! ```
//!
//! Steps per tick are exact and must not change with the build; timings depend on the machine and
//! its load.

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick
#![allow(clippy::cast_precision_loss)] // counts and microseconds into f64 for reporting

use std::time::Instant;

use amoris_script::{CompiledSet, ScriptLimits, ScriptSource, compile};
use amoris_sim::{NoHooks, Sim, SimConfig, TickRate};
use serde_json::{Value, json};

/// Iterations of a probe's loop per tick.
const N: u32 = 200_000;

/// The probes: name, statements before the loop, the loop's body (`i` the index, `s` a number).
const PROBES: [(&str, &str, &str); 11] = [
    ("loop", "", "s += i * 0.001;"),
    ("int", "", "s = (s + i) | 0;"),
    ("call", "", "s += half(i);"),
    (
        "f64",
        "const a = new Float64Array(1024);",
        "const k = i & 1023; a[k] = a[k] + i * 0.5;",
    ),
    ("prop", "const o = { x: 0, y: 0.5 };", "o.x = o.x + o.y;"),
    // QuickJS-ng's own `Math.sqrt` (kept: correctly rounded everywhere), a C function.
    ("sqrt", "", "s += Math.sqrt(i * 0.001);"),
    // The engine's (`amoris_sim::math` through the sandbox).
    ("sin", "", "s += Math.sin(i * 0.001);"),
    ("cos", "", "s += Math.cos(i * 0.001);"),
    ("atan2", "", "s += Math.atan2(i * 0.001, 1.5);"),
    ("pow", "", "s += Math.pow(i * 0.001, 1.5);"),
    ("hypot", "", "s += Math.hypot(i * 0.001, 3);"),
];

fn source(setup: &str, body: &str) -> String {
    format!(
        r#"import {{ game, system }} from "amoris";
function half(x: number): number {{ return x * 0.5; }}
const probe = system({{
    name: "probe", phase: "update", doc: "A microloop.",
    run(ctx) {{
        let s = 0;
        {setup}
        for (let i = 0; i < {N}; i++) {{ {body} }}
        if (s === -1) ctx.emit("micro.never");
    }},
}});
export default game({{ systems: [probe] }});
"#
    )
}

fn compiled(setup: &str, body: &str) -> CompiledSet {
    let src = ScriptSource::new().with("scripts/main.ts", &source(setup, body));
    compile(&src, &Default::default()).unwrap_or_else(|e| panic!("{e:#?}"))
}

fn limits() -> ScriptLimits {
    ScriptLimits {
        steps_per_system: 50_000_000,
        steps_per_tick: 100_000_000,
        ..ScriptLimits::default()
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return f64::NAN;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0 <= p * n <= n
    let rank = (p * n as f64).ceil() as usize;
    sorted[rank.clamp(1, n) - 1]
}

/// One probe's round: a fresh world, the warm-up, the measured ticks.
fn round(name: &str, set: &CompiledSet, warmup: u32, ticks: u32) -> Value {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 1,
    })
    .unwrap_or_else(|e| panic!("{e:?}"));
    amoris_script::install(&mut sim, limits()).unwrap_or_else(|p| panic!("{p:#?}"));
    amoris_script::swap(sim.world_mut(), set, false).unwrap_or_else(|e| panic!("{e:#?}"));
    let step = |sim: &mut Sim| {
        let r = sim
            .step(&mut NoHooks)
            .unwrap_or_else(|p| panic!("{name}: {p:#?}"));
        assert!(r.errors.is_empty(), "{name}: {:#?}", r.errors);
    };
    for _ in 0..warmup {
        step(&mut sim);
    }
    let mut us = Vec::new();
    for _ in 0..ticks {
        let t0 = Instant::now();
        step(&mut sim);
        us.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    let steps: u64 = amoris_script::scripts::last_steps(sim.world())
        .iter()
        .map(|(_, s)| s)
        .sum();
    us.sort_by(f64::total_cmp);
    let median = percentile(&us, 0.5);
    let r = |x: f64| (x * 10.0).round() / 10.0;
    json!({
        "probe": name,
        "n": N,
        "tick_us": {"median": r(median), "min": r(us[0]), "max": r(us[us.len() - 1])},
        "ns_per_iter": (median * 1e3 / f64::from(N) * 100.0).round() / 100.0,
        "steps": steps,
    })
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rounds: usize = arg(&args, "--rounds", 3);
    let offset: usize = arg(&args, "--offset", 0);
    let warmup: u32 = arg(&args, "--warmup", 3);
    let ticks: u32 = arg(&args, "--ticks", 15);
    let all = PROBES.map(|p| p.0).join(",");
    let wanted: Vec<String> = arg(&args, "--probes", all)
        .split(',')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    let probes: Vec<(&str, &str, &str)> = wanted
        .iter()
        .map(|w| {
            *PROBES
                .iter()
                .find(|p| p.0 == w)
                .unwrap_or_else(|| panic!("unknown probe {w}"))
        })
        .collect();
    let json_out: Option<String> = args
        .iter()
        .position(|a| a == "--json")
        .and_then(|i| args.get(i + 1).cloned());
    let run = move || {
        let sets: Vec<(&str, CompiledSet)> = probes
            .iter()
            .map(|(name, setup, body)| (*name, compiled(setup, body)))
            .collect();
        let mut results = Vec::new();
        for r in 0..rounds {
            let k = sets.len();
            for i in 0..k {
                let (name, set) = &sets[(i + r + offset) % k];
                let mut res = round(name, set, warmup, ticks);
                res["round"] = json!(r + 1);
                println!(
                    "round {} {:6} {:>8} ns/iter  tick median {} us  steps {}",
                    r + 1,
                    name,
                    res["ns_per_iter"],
                    res["tick_us"]["median"],
                    res["steps"]
                );
                results.push(res);
            }
        }
        let out = json!({
            "target": "native",
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "qjs_cc": amoris_script::QJS_CC,
            "n": N,
            "warmup": warmup,
            "ticks": ticks,
            "rounds": rounds,
            "results": results,
        });
        if let Some(p) = &json_out {
            std::fs::write(p, serde_json::to_string_pretty(&out).unwrap_or_default())
                .unwrap_or_else(|e| panic!("{p}: {e}"));
            println!("wrote {p}");
        }
    };
    std::thread::Builder::new()
        .stack_size(amoris_script::host::THREAD_STACK_BYTES)
        .spawn(run)
        .expect("a thread")
        .join()
        .expect("the bench");
}
