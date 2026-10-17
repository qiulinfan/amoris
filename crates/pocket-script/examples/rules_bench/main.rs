//! The script cost baseline (docs/bench/script.md; charter 4.2 and 8, item 1): the rule workload
//! of `bench/rules` (docs/spec/budgets.md 4, `tick.rules`) through the real host (`install`,
//! `swap`, `Sim::step`), beside the same rule in Rust (the twin) and two probes that isolate the
//! fixed cost of a script call:
//!
//! - **rules**: `populate` at tick 1, then `damage` every tick (1,000 rows, eight columns, two
//!   events);
//! - **twin**: the same two systems in Rust over Rust components (`workload.rs`), which must build
//!   the same world: `--verify` compares the digests of both at every tick first;
//! - **idle**: `populate`, then a system with no query and an empty `run`;
//! - **scan**: `populate`, then a system with `damage`'s query and an empty `run`.
//!
//! Each round builds a fresh world per variant (in an order that rotates by round, from
//! `--offset`), steps it `--warmup` ticks, then times `--ticks` ticks one by one: the tick
//! (`Sim::step` alone), the `script.update` system and the measured script system (step hooks),
//! the script call's phases (`ScriptHost::time_phases`) and its steps.
//!
//! ```text
//! cargo run --release -p pocket-script --features transpile --example rules_bench -- \
//!     [--rounds 3] [--offset 0] [--warmup 120] [--ticks 600] \
//!     [--variants rules,twin,idle,scan] [--no-verify] [--json out.json] [--emit DIR] [--types]
//! ```
//!
//! `--emit DIR` writes the compiled sets and the verified digests for the WebAssembly module
//! (`rules_web`, `bench/rules/web/`) and exits, failing when the twin differs; `--types` writes `bench/rules/.pocket/types` for
//! `tsc`. Timings depend on the machine and its load.

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick
#![allow(clippy::cast_precision_loss)] // counts and microseconds into f64 for reporting

mod workload;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use pocket_script::{CompiledSet, ScriptSource, compile};
use serde_json::{Value, json};
use workload::{Bench, Sample};

const VARIANTS: [&str; 4] = ["rules", "twin", "idle", "scan"];

/// The entry module of a probe: `populate`, then the probe in place of `damage`.
const PROBE_MAIN: &str = r#"import { game } from "pocket";
import { Armor, Attack, Health, Spot, Stats } from "./components";
import { populate } from "./rules";
import { PROBE } from "./probes";

export default game({
    components: [Attack, Spot, Health, Armor, Stats],
    systems: [populate, PROBE],
});
"#;

fn now_ms() -> f64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64() * 1e3
}

fn project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/rules")
}

/// The compiled set of a script variant.
fn compiled(variant: &str) -> CompiledSet {
    let mut src = ScriptSource::read_dir(&project()).unwrap_or_else(|e| panic!("bench/rules: {e}"));
    if variant != "rules" {
        src = src.with("scripts/main.ts", &PROBE_MAIN.replace("PROBE", variant));
    }
    compile(&src, &Default::default()).unwrap_or_else(|e| panic!("{variant}: {e:#?}"))
}

/// The measured script system of a variant.
fn measured(variant: &str) -> &str {
    if variant == "rules" {
        "damage"
    } else {
        variant
    }
}

fn bench(variant: &str, sets: &[(String, CompiledSet)]) -> Bench {
    let made = if variant == "twin" {
        Bench::twin(now_ms)
    } else {
        let set = &sets.iter().find(|(v, _)| v == variant).expect("compiled").1;
        Bench::script(set, measured(variant), now_ms)
    };
    made.unwrap_or_else(|e| panic!("{variant}: {e}"))
}

/// The digests of the script world and the twin at every tick: equal, or the first tick that
/// differs. Returns the report and the final digest.
fn verify(sets: &[(String, CompiledSet)], ticks: u64) -> (Value, u64) {
    let mut script = bench("rules", sets);
    let mut twin = bench("twin", sets);
    let mut chain = 0xcbf2_9ce4_8422_2325u64;
    let mut combat = [0u64; 3];
    for t in 1..=ticks {
        script.tick().unwrap_or_else(|e| panic!("rules: {e}"));
        twin.tick().unwrap_or_else(|e| panic!("twin: {e}"));
        for (sum, x) in combat.iter_mut().zip(script.combat()) {
            *sum += x;
        }
        let (a, b) = (script.digest(), twin.digest());
        if a != b {
            return (
                json!({"ticks": ticks, "identical": false, "first_divergent_tick": t,
                       "script": format!("{a:016x}"), "twin": format!("{b:016x}")}),
                a,
            );
        }
        chain = (chain ^ a).wrapping_mul(0x0000_0100_0000_01b3);
    }
    let last = script.digest();
    (
        json!({"ticks": ticks, "identical": true, "final": format!("{last:016x}"),
               "chain": format!("{chain:016x}"), "damaged_events": combat[0], "hits": combat[1],
               "kills": combat[2]}),
        last,
    )
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

fn stats(v: &[f64]) -> Value {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let mean = s.iter().sum::<f64>() / s.len().max(1) as f64;
    let r = |x: f64| (x * 10.0).round() / 10.0;
    json!({"median": r(percentile(&s, 0.5)), "min": r(s.first().copied().unwrap_or(f64::NAN)),
           "p99": r(percentile(&s, 0.99)), "max": r(s.last().copied().unwrap_or(f64::NAN)),
           "mean": r(mean)})
}

/// One variant's round: a fresh world, the warm-up, the measured ticks.
fn round(variant: &str, sets: &[(String, CompiledSet)], warmup: u64, ticks: u64) -> Value {
    let mut b = bench(variant, sets);
    for _ in 0..warmup {
        b.tick().unwrap_or_else(|e| panic!("{variant}: {e}"));
        let _ = b.sample();
    }
    let n = usize::try_from(ticks).unwrap_or(0);
    let mut wall = Vec::with_capacity(n);
    let mut samples: Vec<Sample> = Vec::with_capacity(n);
    for _ in 0..ticks {
        let t0 = Instant::now();
        b.tick().unwrap_or_else(|e| panic!("{variant}: {e}"));
        wall.push(t0.elapsed().as_secs_f64() * 1e6);
        samples.push(b.sample());
    }
    let us = |f: fn(&Sample) -> f64| samples.iter().map(|s| f(s) * 1e3).collect::<Vec<f64>>();
    let steps: Vec<f64> = samples.iter().map(|s| s.steps as f64).collect();
    json!({
        "variant": variant,
        "ticks": ticks,
        "after_tick": b.tick_count(),
        "tick_us": stats(&wall),
        "update_us": stats(&us(|s| s.update)),
        "system_us": stats(&us(|s| s.system)),
        "phases_us": {
            "prepare": stats(&us(|s| s.phases.prepare)),
            "run": stats(&us(|s| s.phases.run)),
            "diff": stats(&us(|s| s.phases.diff)),
            "apply": stats(&us(|s| s.phases.apply)),
        },
        "steps": stats(&steps),
        "digest": format!("{:016x}", b.digest()),
        "raw_tick_us": wall.iter().map(|x| (x * 10.0).round() / 10.0).collect::<Vec<f64>>(),
    })
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// `bench/rules/.pocket/types`: the SDK's declarations and the bench's components, for `tsc`.
fn write_types(set: &CompiledSet) {
    let b = Bench::script(set, "damage", now_ms).unwrap_or_else(|e| panic!("{e}"));
    let dir = project().join(".pocket/types");
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    let world = b.world();
    let (usable, other) = pocket_script::types::usable_components(world);
    let usable: Vec<&pocket_sim::registry::ComponentSchema> = usable.iter().map(|s| &**s).collect();
    let other: Vec<&str> = other.iter().map(String::as_str).collect();
    for (name, text) in [
        ("pocket.d.ts", pocket_script::types::pocket_dts()),
        (
            "components.d.ts",
            pocket_script::types::components_dts(&usable, &other),
        ),
    ] {
        std::fs::write(dir.join(name), text).unwrap_or_else(|e| panic!("{e}"));
    }
    println!("wrote {}", dir.display());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rounds: usize = arg(&args, "--rounds", 3);
    let offset: usize = arg(&args, "--offset", 0);
    let warmup: u64 = arg(&args, "--warmup", 120);
    let ticks: u64 = arg(&args, "--ticks", 600);
    let variants: Vec<String> = arg(&args, "--variants", VARIANTS.join(","))
        .split(',')
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect();
    for v in &variants {
        assert!(VARIANTS.contains(&v.as_str()), "unknown variant {v}");
    }
    let json_out: Option<String> = args
        .iter()
        .position(|a| a == "--json")
        .and_then(|i| args.get(i + 1).cloned());
    let emit: Option<String> = args
        .iter()
        .position(|a| a == "--emit")
        .and_then(|i| args.get(i + 1).cloned());
    let run = move || {
        let sets: Vec<(String, CompiledSet)> = ["rules", "idle", "scan"]
            .iter()
            .map(|v| ((*v).to_owned(), compiled(v)))
            .collect();
        if has(&args, "--types") {
            write_types(&sets[0].1);
            return;
        }
        if let Some(dir) = emit {
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
            for (v, set) in &sets {
                std::fs::write(dir.join(format!("{v}.json")), set.to_json())
                    .unwrap_or_else(|e| panic!("{e}"));
            }
            let (report, _) = verify(&sets, warmup + ticks);
            std::fs::write(dir.join("verify.json"), report.to_string())
                .unwrap_or_else(|e| panic!("{e}"));
            println!("{report}");
            // The timing runs that follow pass `--no-verify`: this is their verification.
            assert_eq!(
                report["identical"],
                json!(true),
                "the twin differs: {report}"
            );
            return;
        }
        let verified = if has(&args, "--no-verify") {
            Value::Null
        } else {
            let (report, _) = verify(&sets, warmup + ticks);
            println!("verify {report}");
            assert_eq!(
                report["identical"],
                json!(true),
                "the twin differs: {report}"
            );
            report
        };
        let mut results = Vec::new();
        for r in 0..rounds {
            let k = variants.len();
            for i in 0..k {
                let v = &variants[(i + r + offset) % k];
                let mut res = round(v, &sets, warmup, ticks);
                res["round"] = json!(r + 1);
                println!(
                    "round {} {:5} tick median {} us min {} p99 {} | system {} | steps {}",
                    r + 1,
                    v,
                    res["tick_us"]["median"],
                    res["tick_us"]["min"],
                    res["tick_us"]["p99"],
                    res["system_us"]["median"],
                    res["steps"]["median"]
                );
                results.push(res);
            }
        }
        let out = json!({
            "target": "native",
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "qjs_cc": pocket_script::QJS_CC,
            "warmup": warmup,
            "ticks": ticks,
            "rounds": rounds,
            "verify": verified,
            "results": results,
        });
        if let Some(p) = &json_out {
            std::fs::write(p, serde_json::to_string_pretty(&out).unwrap_or_default())
                .unwrap_or_else(|e| panic!("{p}: {e}"));
            println!("wrote {p}");
        }
    };
    std::thread::Builder::new()
        .stack_size(pocket_script::host::THREAD_STACK_BYTES)
        .spawn(run)
        .expect("a thread")
        .join()
        .expect("the bench");
}
