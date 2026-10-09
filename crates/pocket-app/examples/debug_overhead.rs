//! What the script debugger costs (docs/spec/debugger.md 10): the script host's web workload (40
//! boats, 160 crates, 7 systems; crates/pocket-script/tests/web/workload) run as a game, timed per
//! tick (the game thread's CPU time) in each debugger mode, the modes interleaved round by round.
//!
//! ```sh
//! cargo run --release -p pocket-app --example debug_overhead -- [--rounds 7] [--ticks 600] [--json]
//! ```
//!
//! Modes: `none` (no debugger attached to the game), `detached` (a hub's hook attached, no
//! frontend: the program is not instrumented), `attached` (a frontend attached: every statement
//! calls the trace hook, no breakpoint), `armed` (attached, a breakpoint on a line no tick runs),
//! `watch` (attached, a data breakpoint on a field no system writes, probed at every statement).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pocket_debug::{DebugHub, HubOptions};
use pocket_link::Source;
use pocket_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};

const MODES: &[&str] = &["none", "detached", "attached", "armed", "watch"];

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// This thread's CPU time in microseconds: on a machine shared with other work, wall time
/// measures the scheduler as much as the code.
#[cfg(unix)]
fn cpu_us() -> f64 {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, tp: *mut Timespec) -> i32;
    }
    #[cfg(target_os = "macos")]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 16;
    #[cfg(not(target_os = "macos"))]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    let mut t = Timespec { sec: 0, nsec: 0 };
    unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut t) };
    t.sec as f64 * 1e6 + t.nsec as f64 / 1e3
}

/// This thread's CPU time in microseconds (user plus kernel, in 100 ns ticks on Windows).
#[cfg(windows)]
fn cpu_us() -> f64 {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut std::ffi::c_void;
        fn GetThreadTimes(
            thread: *mut std::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    let ticks = |t: &FileTime| (u64::from(t.high) << 32 | u64::from(t.low)) as f64;
    let (mut c, mut e, mut k, mut u) = Default::default();
    unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) };
    (ticks(&k) + ticks(&u)) / 10.0
}

/// The workload as a project: its scripts, an empty scene.
fn project(dir: &Path) -> Project {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scripts = root.join("crates/pocket-script/tests/web/workload/scripts");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    for e in std::fs::read_dir(&scripts).unwrap() {
        let e = e.unwrap();
        // The workload runs on a bare simulation; in a game, `Boat` and `Wind` are pocket-physics' components.
        let text = std::fs::read_to_string(e.path())
            .unwrap()
            .replace("Boat", "Vessel")
            .replace("Wind", "Breeze")
            .replace("Crate", "Cask");
        std::fs::write(dir.join("scripts").join(e.file_name()), text).unwrap();
    }
    std::fs::write(
        dir.join("project.toml"),
        "name = \"workload\"\nrate = 60\nseed = 1\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("scene.json"),
        "{\"format\": \"pocket-scene\", \"version\": 1, \"entities\": []}\n",
    )
    .unwrap();
    Project::load(dir).unwrap()
}

/// The lowest entity id with a Vessel (the fleet spawns at tick 1).
fn first_boat(game: &mut Game) -> u64 {
    (1..1000)
        .find(|id| {
            let cmd = Command::new(
                Source::Developer(0),
                *id,
                "world_get",
                json!({"entity": id, "components": ["Vessel"]}),
            );
            game.apply(&cmd)
                .is_ok_and(|v| v["components"].get("Vessel").is_some())
        })
        .expect("a boat")
}

/// One mode's game, its debugger set up, warmed up.
fn game(setup: &Arc<GameSetup>, mode: &str) -> (Game, DebugHub) {
    let mut game = Game::new(setup.clone(), 1).unwrap();
    let hub = DebugHub::new(HubOptions::default());
    if mode != "none" {
        game.set_script_debugger(Some(hub.hook()));
    }
    // The setup system spawns the fleet at tick 1.
    for _ in 0..2 {
        game.step().unwrap();
    }
    match mode {
        "attached" => {
            hub.call("debug.attach", &json!({})).unwrap();
        }
        "armed" => {
            // A breakpoint whose condition never holds, on a line that runs only when crates run
            // low (`collect` drops more): the breakpoint tables are armed, nothing stops.
            hub.call(
                "debug.breakpoints.set",
                &json!({"file": "scripts/sailing.ts", "line": 94, "condition": "false"}),
            )
            .unwrap();
        }
        "watch" => {
            // The first vessel's rig, which no system writes: probed at every statement, never hit.
            let boat = first_boat(&mut game);
            hub.call(
                "debug.watch",
                &json!({"entity": boat, "component": "Vessel", "field": "rig"}),
            )
            .unwrap();
        }
        _ => {}
    }
    for _ in 0..60 {
        game.step().unwrap();
    }
    (game, hub)
}

/// Asks the scheduler for a performance core (macOS): an efficiency core runs the same code at a
/// fraction of the speed, which would swamp what is measured.
#[cfg(target_os = "macos")]
fn prefer_performance_cores() {
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos: u32, priority: i32) -> i32;
    }
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
    unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
}

#[cfg(not(target_os = "macos"))]
fn prefer_performance_cores() {}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rounds: usize = value(&args, "--rounds")
        .and_then(|r| r.parse().ok())
        .unwrap_or(7);
    let ticks: usize = value(&args, "--ticks")
        .and_then(|r| r.parse().ok())
        .unwrap_or(600);
    let dir = std::env::temp_dir().join(format!("pocket-debug-overhead-{}", std::process::id()));
    let p = project(&dir);
    let setup = Arc::new(p.setup(false).unwrap());
    prefer_performance_cores();
    // Every mode's game steps in turn, tick by tick, so what the machine does meanwhile falls on
    // all of them alike; a round is `ticks` ticks of each, from fresh games.
    let mut per_mode: Vec<Vec<f64>> = vec![Vec::new(); MODES.len()];
    let mut ratios: Vec<Vec<f64>> = vec![Vec::new(); MODES.len()];
    let mut statements = 0u64;
    for round in 0..rounds {
        let mut games: Vec<(Game, DebugHub)> = MODES.iter().map(|m| game(&setup, m)).collect();
        let before: Vec<u64> = games.iter().map(|(_, h)| h.traced_statements()).collect();
        let mut times: Vec<Vec<f64>> = vec![Vec::with_capacity(ticks); MODES.len()];
        for t in 0..ticks {
            for k in 0..MODES.len() {
                let i = (k + t + round) % MODES.len();
                let start = cpu_us();
                games[i].0.step().unwrap();
                times[i].push(cpu_us() - start);
            }
        }
        for (i, (_, hub)) in games.iter().enumerate() {
            assert!(!hub.is_paused());
            if MODES[i] == "attached" {
                statements = (hub.traced_statements() - before[i]) / ticks as u64;
            }
        }
        let meds: Vec<f64> = times.iter_mut().map(|t| median(t)).collect();
        for i in 0..MODES.len() {
            per_mode[i].push(meds[i]);
            ratios[i].push(meds[i] / meds[0]);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    let base = median(&mut per_mode[0].clone());
    let rows: Vec<Value> = MODES
        .iter()
        .zip(per_mode.iter_mut())
        .map(|(m, v)| {
            let med = median(v);
            let best = v.iter().copied().fold(f64::INFINITY, f64::min);
            let ratio = median(&mut ratios[MODES.iter().position(|x| x == m).unwrap_or(0)].clone());
            let per_statement = if statements > 0 {
                (ratio - 1.0) * base * 1000.0 / statements as f64
            } else {
                0.0
            };
            json!({"mode": m, "us_per_tick": (med * 10.0).round() / 10.0,
                   "versus_none": (ratio * 1000.0).round() / 1000.0,
                   "ns_per_statement": (per_statement * 10.0).round() / 10.0,
                   "best_us_per_tick": (best * 10.0).round() / 10.0,
                   "rounds": v.iter().map(|x| (x * 10.0).round() / 10.0).collect::<Vec<_>>()})
        })
        .collect();
    if args.iter().any(|a| a == "--json") {
        println!(
            "{}",
            json!({"ticks": ticks, "rounds": rounds, "statements_per_tick": statements, "modes": rows})
        );
    } else {
        println!("{statements} statements per tick traced when attached");
        println!(
            "| Mode | CPU us per tick (median over {rounds} rounds of {ticks} interleaved ticks) | Best round | Versus none (median of the rounds' ratios) | Extra ns per statement |"
        );
        println!("|---|---|---|---|---|");
        for r in &rows {
            println!(
                "| {} | {} | {} | {} | {} |",
                r["mode"].as_str().unwrap_or(""),
                r["us_per_tick"],
                r["best_us_per_tick"],
                r["versus_none"],
                r["ns_per_statement"]
            );
        }
    }
}
