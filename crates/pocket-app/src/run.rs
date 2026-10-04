//! `pocket run <project> --headless` (architecture.md 4.13): the project's game on its game thread
//! (threads.md 3), driven by a developer client like any agent: stepped (`--ticks N`, the default
//! 600) or in real time (`--realtime SPEED --seconds S`), with the inputs of a file applied at their
//! ticks. It prints where every named body is and what the run saw, from the last snapshot it read.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use pocket_contract::{Problem, detail};
use pocket_link::{EventCursor, SnapshotView, Source};
use pocket_runtime::thread::{GameThread, Pacing, ThreadOptions};
use pocket_runtime::{Game, Project};
use serde_json::{Value, json};

use crate::check::Outcome;
use crate::cli::{Args, Flag};

const FLAGS: &[Flag] = &[
    ("headless", false),
    ("json", false),
    ("seed", true),
    ("ticks", true),
    ("inputs", true),
    ("realtime", true),
    ("seconds", true),
];

fn fail(p: &Problem, code: i32) -> Outcome {
    Outcome {
        stdout: serde_json::to_string_pretty(&json!({"error": p})).unwrap_or_default(),
        code,
    }
}

/// `pocket run`.
pub fn run(raw: &[String]) -> Outcome {
    let args = match Args::parse(raw, FLAGS) {
        Ok(a) => a,
        Err(p) => return fail(&p, 2),
    };
    if !args.has("headless") {
        return fail(
            &Problem::new(
                "check.usage",
                "Slice 1 runs games headless only: pocket run <project> --headless [--ticks N] [--seed S] [--inputs FILE] [--realtime SPEED --seconds S] [--json]",
                detail([]),
            ),
            2,
        );
    }
    if let Err(p) = timing(&args) {
        return fail(&p, 2);
    }
    match headless(&args) {
        Ok(v) => Outcome {
            stdout: if args.has("json") {
                serde_json::to_string_pretty(&v).unwrap_or_default()
            } else {
                summary(&v)
            },
            code: 0,
        },
        Err(p) => fail(&p, 1),
    }
}

/// Refuses a real-time speed the time model would refuse (`request.out_of_range`, `Pacing::check`)
/// and a `--seconds` that is not a finite number of at least 0, before the game starts.
fn timing(args: &Args) -> Result<(), Problem> {
    if let Some(speed) = args.number::<f64>("realtime")? {
        Pacing::RealTime { speed }.check()?;
    }
    if let Some(s) = args.number::<f64>("seconds")?
        && !(s.is_finite() && s >= 0.0)
    {
        return Err(crate::cli::usage(
            format!("--seconds takes a finite number of at least 0, not '{s}'"),
            "seconds",
            &[],
        ));
    }
    Ok(())
}

fn summary(v: &Value) -> String {
    let mut out = format!(
        "tick {} world {} ({} events)\n",
        v["tick"], v["world_hash"], v["events"]
    );
    if let Some(bodies) = v["bodies"].as_array() {
        for b in bodies {
            out.push_str(&format!("{} {}\n", b["name"], b["position"]));
        }
    }
    out
}

fn headless(args: &Args) -> Result<Value, Problem> {
    let dir = args.positional.first().map(PathBuf::from).ok_or_else(|| {
        Problem::new(
            "check.usage",
            "pocket run needs a project directory",
            detail([]),
        )
    })?;
    let project = Project::load(&dir)?;
    let seed = args.number::<u64>("seed")?.unwrap_or(project.manifest.seed);
    let ticks = args.number::<u64>("ticks")?.unwrap_or(600);
    let inputs = pocket_check::Inputs::load(&dir, args.value("inputs"))?;
    let setup = Arc::new(project.setup(false)?);
    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let options = ThreadOptions::new(clock);
    let realtime = args.number::<f64>("realtime")?;
    let handle = GameThread::spawn(move || Game::new(setup, seed), options)?;
    let reader = handle.reader();
    let mut dev = handle.developer();
    let mut players: Vec<(Source, pocket_link::GameClient)> = Vec::new();
    let mut events = EventCursor::start();
    let mut seen = 0u64;
    if let Some(speed) = realtime {
        let seconds = args.number::<f64>("seconds")?.unwrap_or(10.0);
        dev.call(
            "time_control",
            json!({"pacing": {"real_time": {"speed": speed}}}),
        )?;
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
        dev.call("time_control", json!({"pause": true}))?;
    } else {
        // Inputs go at their ticks (threads.md 5.2: an input of tick T applies at boundary T - 1).
        let mut t = 0;
        while t < ticks {
            let next = t + 1;
            for i in inputs.at(next) {
                let client = match players.iter_mut().find(|(s, _)| *s == i.source) {
                    Some((_, c)) => c,
                    None => {
                        let c = if i.source == dev.source() {
                            None
                        } else {
                            Some(handle.client(i.source)?)
                        };
                        match c {
                            Some(c) => {
                                players.push((i.source, c));
                                &mut players.last_mut().expect("pushed").1
                            }
                            None => &mut dev,
                        }
                    }
                };
                client
                    .call_at(&i.name, i.params.clone(), Some(pocket_sim::Tick(next)))
                    .map_err(|e| pocket_check::runs::input_refused(i.line, next, &e))?;
            }
            dev.call("step", json!({"ticks": 1}))?;
            seen += reader.events(&mut events, usize::MAX).records.len() as u64;
            t = next;
        }
    }
    seen += reader.events(&mut events, usize::MAX).records.len() as u64;
    let snap = reader.latest();
    let view = SnapshotView::new(&snap);
    let names: Vec<(pocket_sim::EntityId, Value)> = view
        .entities()?
        .into_iter()
        .filter_map(|e| Some((e, view.get_json(e, "Transform").ok()??)))
        .collect();
    let named = view.column::<pocket_sim::Name>()?;
    let bodies: Vec<Value> = names
        .iter()
        .map(|(id, t)| {
            let name = named.iter().find(|(i, _)| i == id).map(|(_, n)| n.as_str());
            json!({"id": id.get(), "name": name, "position": t["position"]})
        })
        .collect();
    let status = dev.call("status", json!({}))?.into_json();
    drop(players);
    drop(dev);
    handle.shutdown(2000)?;
    Ok(json!({
        "project": project.manifest.name,
        "seed": seed,
        "tick": snap.state().0,
        "world_hash": snap.snapshot.world_hash().to_string(),
        "status": status,
        "events": seen,
        "publications": snap.version,
        "time": snap.time,
        "bodies": bodies,
    }))
}
