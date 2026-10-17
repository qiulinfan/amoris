//! The write trace's bench (docs/bench/write-trace.md): what a `time.step {trace}` costs a tick,
//! on a project's game outside the game thread.
//!
//! Modes, each in blocks of `ticks` ticks timed one by one, the blocks of one round in rotating
//! order (A B C D, then B C D A, ...) on one world, so drift and the world's own evolution fall on
//! every mode alike:
//!
//! - **off**: `Game::step`.
//! - **probe F**, for each `--field` F: the script host's recorder alone
//!   (`Game::record_script_writes`): the program instrumented and F's staged value probed at every
//!   statement of every script system call (a component scripts cannot read fails once a call).
//! - **trace F**, for each `--field` F: the whole trace (`WriteTrace::step`): the recorder when
//!   scripts can read F, the boundary's writes, and F read after every system.
//!
//! The first tick of each block (the program instantiated again, instrumented or not, when the
//! mode before it differs) is timed apart: `first_us`, the switch into the mode. Medians and means
//! per tick, and each mode's median against `off`. Before the rounds, one untimed block counts the
//! script statements a tick runs (a debugger hook that only counts), the probe's work per tick.
//!
//! `cargo run --release -p amoris-runtime --features transpile --example write_trace_bench -- \
//!  [--ticks 600] [--rounds 5] [--warmup 120] [--field Sloop.Boat.rudder].. [--out out.json] \
//!  [project]`; an entity is a name or an id (`7.Health.hp`). `tools/write_trace_bench.py` runs
//! it under the bench lock with the machine's conditions. Timings depend on the machine and its
//! load: the bench doc says under which they were taken.

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick
#![allow(clippy::cast_precision_loss)] // counts and microseconds into f64 for reporting

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use amoris_runtime::writes::{Trace, WriteTrace};
use amoris_runtime::{EntityRef, Game, GameBuilder, Project};
use amoris_script::CompiledSet;
use amoris_script::debug::{DebugHook, FieldWatch, TraceKind};
use serde_json::{Value, json};

struct Options {
    ticks: u64,
    rounds: u64,
    warmup: u64,
    fields: Vec<String>,
    out: Option<PathBuf>,
    project: PathBuf,
}

fn options() -> Options {
    let mut o = Options {
        ticks: 600,
        rounds: 5,
        warmup: 120,
        fields: Vec::new(),
        out: None,
        project: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing"),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut num = |name: &str| -> u64 {
            args.next()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("{name} takes a number"))
        };
        match a.as_str() {
            "--ticks" => o.ticks = num("--ticks").max(2),
            "--rounds" => o.rounds = num("--rounds").max(1),
            "--warmup" => o.warmup = num("--warmup"),
            "--field" => o.fields.push(args.next().expect("--field takes E.C.f")),
            "--out" => o.out = args.next().map(PathBuf::from),
            p => o.project = PathBuf::from(p),
        }
    }
    if o.fields.is_empty() {
        o.fields = vec![
            "Sloop.Boat.rudder".into(),
            "Sloop.Log.distance".into(),
            "Sloop.Model".into(),
        ];
    }
    o
}

/// `Entity.Component.field` (or `Entity.Component`) as a trace; an entity of digits is an id.
fn trace_of(spec: &str) -> Trace {
    let mut parts = spec.splitn(3, '.');
    let (Some(e), Some(c)) = (parts.next(), parts.next()) else {
        panic!("{spec} is not Entity.Component.field");
    };
    Trace {
        entity: e
            .parse()
            .map_or_else(|_| EntityRef::Name(e.to_owned()), |n: u64| EntityRef::Id(n)),
        component: c.to_owned(),
        field: parts.next().map(str::to_owned),
    }
}

#[derive(Clone)]
enum Mode {
    Off,
    Probe(String),
    Trace(String),
}

impl Mode {
    fn name(&self) -> String {
        match self {
            Mode::Off => "off".into(),
            Mode::Probe(f) => format!("probe {f}"),
            Mode::Trace(f) => format!("trace {f}"),
        }
    }
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Microseconds `f` takes.
fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let r = f();
    (r, t.elapsed().as_secs_f64() * 1e6)
}

/// One block of `mode`.
struct Block {
    /// The first tick's time and the `ticks - 1` timed after it, in microseconds.
    first: f64,
    times: Vec<f64>,
    /// The rows the trace answered (the statements the probe recorded), and the trace's last row.
    rows: u64,
    last_row: Value,
}

fn block(game: &mut Game, mode: &Mode, ticks: u64) -> Block {
    let mut times = Vec::new();
    let mut rows = 0;
    let mut last_row = Value::Null;
    let first;
    match mode {
        Mode::Off => {
            first = timed(|| game.step().unwrap()).1;
            for _ in 1..ticks {
                times.push(timed(|| game.step().unwrap()).1);
            }
        }
        Mode::Probe(spec) => {
            let t = trace_of(spec);
            let id =
                amoris_runtime::resolve(game.world(), &t.entity, &amoris_contract::Pointer::root())
                    .unwrap();
            let field = t.field.as_deref().and_then(|f| f.split('.').next());
            let w = FieldWatch::new(id.get(), &t.component, field).unwrap();
            game.record_script_writes(Some(w));
            first = timed(|| game.step().unwrap()).1;
            for _ in 1..ticks {
                times.push(timed(|| game.step().unwrap()).1);
                rows += game.take_script_writes().0.len() as u64;
            }
            game.record_script_writes(None);
        }
        Mode::Trace(spec) => {
            let mut trace = WriteTrace::new(game, &trace_of(spec)).unwrap();
            first = timed(|| {
                trace
                    .step(game, &mut amoris_sim::NoHooks)
                    .unwrap_or_else(|e| panic!("{e:#?}"))
            })
            .1;
            for _ in 1..ticks {
                times.push(timed(|| trace.step(game, &mut amoris_sim::NoHooks).unwrap()).1);
            }
            let w = trace.finish(game);
            rows = w["rows"].as_array().map_or(0, |r| r.len() as u64);
            last_row = w["rows"]
                .as_array()
                .and_then(|r| r.last())
                .cloned()
                .unwrap_or_default();
        }
    }
    Block {
        first,
        times,
        rows,
        last_row,
    }
}

/// A debugger hook that only counts the statements traced.
struct Counter(Cell<u64>);

impl DebugHook for Counter {
    fn instrument(&self) -> bool {
        true
    }

    fn loaded(&self, _set: &Arc<CompiledSet>, _instrumented: bool) {}

    fn trace(&self, trace: &amoris_script::debug::Trace) {
        if trace.kind == TraceKind::Statement {
            self.0.set(self.0.get() + 1);
        }
    }
}

/// The script statements `game` runs a tick, over `ticks` ticks (untimed).
fn statements_per_tick(game: &mut Game, ticks: u64) -> f64 {
    let counter = Rc::new(Counter(Cell::new(0)));
    game.set_script_debugger(Some(counter.clone()));
    game.step().unwrap();
    counter.0.set(0);
    game.run(ticks).unwrap();
    game.set_script_debugger(None);
    game.step().unwrap();
    counter.0.get() as f64 / ticks.max(1) as f64
}

fn main() {
    let o = options();
    let handle = std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(move || run(&o))
        .unwrap();
    handle.join().unwrap();
}

fn run(o: &Options) {
    let project = Project::load(&o.project).unwrap_or_else(|e| panic!("{e:#?}"));
    let setup = Arc::new(project.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    let mut game = GameBuilder::new(setup).build().unwrap();
    game.run(o.warmup).unwrap();
    let statements = statements_per_tick(&mut game, o.ticks.min(300));
    eprintln!("{statements:.0} script statements a tick");
    let mut modes = vec![Mode::Off];
    modes.extend(o.fields.iter().map(|f| Mode::Probe(f.clone())));
    modes.extend(o.fields.iter().map(|f| Mode::Trace(f.clone())));
    let mut times: Vec<Vec<f64>> = vec![Vec::new(); modes.len()];
    let mut firsts: Vec<Vec<f64>> = vec![Vec::new(); modes.len()];
    let mut rows: Vec<u64> = vec![0; modes.len()];
    let mut last_rows: Vec<Value> = vec![Value::Null; modes.len()];
    for round in 0..o.rounds {
        for k in 0..modes.len() {
            let i = (k + round as usize) % modes.len();
            let b = block(&mut game, &modes[i], o.ticks);
            firsts[i].push(b.first);
            times[i].extend(b.times);
            rows[i] += b.rows;
            last_rows[i] = b.last_row;
        }
        eprintln!(
            "round {} of {} at tick {}",
            round + 1,
            o.rounds,
            game.tick().0
        );
    }
    let mut out = Vec::new();
    let off = median(&mut times[0].clone());
    println!("{statements:.0} script statements a tick");
    println!(
        "{:<32} {:>10} {:>10} {:>8} {:>10} {:>8}",
        "mode", "median us", "mean us", "vs off", "first us", "rows"
    );
    for (i, m) in modes.iter().enumerate() {
        let mean = times[i].iter().sum::<f64>() / times[i].len().max(1) as f64;
        let med = median(&mut times[i]);
        let first = median(&mut firsts[i]);
        let ratio = if off > 0.0 { med / off } else { 0.0 };
        println!(
            "{:<32} {med:>10.1} {mean:>10.1} {ratio:>8.3} {first:>10.1} {:>8}",
            m.name(),
            rows[i]
        );
        out.push(json!({"mode": m.name(), "median_us": med, "mean_us": mean,
                        "vs_off": ratio, "first_us": first, "ticks": times[i].len(),
                        "rows": rows[i], "last_row": last_rows[i]}));
    }
    let report: Value = json!({
        "project": o.project.display().to_string(),
        "ticks_per_block": o.ticks, "rounds": o.rounds, "warmup": o.warmup,
        "statements_per_tick": statements,
        "modes": out,
    });
    if let Some(path) = &o.out {
        std::fs::write(path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
}
