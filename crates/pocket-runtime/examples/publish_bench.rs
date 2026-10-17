//! The serve loop's publication bench (docs/bench/profiler.md, "The serve loop"): what the game
//! thread does after each tick under `pocket serve`, part by part, on a project's game outside the
//! game thread.
//!
//! After every tick (`Game::step`), timed one by one:
//!
//! - **streams**: the tick's inbox events encoded for the event stream and the scripts' console
//!   lines taken (the loop's `push_events` and `push_log`);
//! - **present**: the render feed's extraction into `--subs` subscribers (the capture server's
//!   mailbox, never taken, and editor viewports' mailboxes, taken after each tick outside the
//!   timing, as `/render` takes them);
//! - **snapshot**: the world's snapshot as the game thread takes it, through pocket-persist's
//!   `SnapshotCache` (`--full`: `Game::snapshot`, every section encoded);
//! - **publish**: the snapshot published through a pocket-link publisher (the swap, and the drop
//!   of the publication before it).
//!
//! Paced (`--pace-ms 16.667`, the default) a tick runs every that many milliseconds and the thread
//! sleeps in slices of at most 1 ms between them, as the game thread does in real time; `--pace-ms
//! 0` runs them back to back. Then the snapshot's anatomy, back to back on the last world: the
//! snapshot, its section digests alone, the registry's checks, hashing alone, and the sections.
//!
//! `cargo run --release -p pocket-runtime --features transpile --example publish_bench -- \
//!  [--ticks 1800] [--warmup 120] [--pace-ms 16.667] [--subs 1] [--repeat 2000] [--out out.json] \
//!  [project]`. Timings depend on the machine and its load: the bench doc says under which they
//! were taken.

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick
#![allow(clippy::cast_precision_loss)] // counts and microseconds into f64 for reporting

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pocket_assets::Feed;
use pocket_link::{PacingStatus, TimeStatus, WorldSnapshot, publication};
use pocket_persist::hash::{digest_prefix, section_digest};
use pocket_runtime::{Extractor, Game, GameBuilder, Project};
use serde_json::{Value, json};

struct Options {
    ticks: u64,
    warmup: u64,
    pace_ms: f64,
    subs: usize,
    full: bool,
    repeat: u32,
    out: Option<PathBuf>,
    project: PathBuf,
}

fn options() -> Options {
    let mut o = Options {
        ticks: 1800,
        warmup: 120,
        pace_ms: 1000.0 / 60.0,
        subs: 1,
        full: false,
        repeat: 2000,
        out: None,
        project: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing"),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| -> String {
            args.next()
                .unwrap_or_else(|| panic!("{name} takes a value"))
        };
        let num = |name: &str, v: String| -> f64 {
            v.parse()
                .unwrap_or_else(|_| panic!("{name} takes a number"))
        };
        match a.as_str() {
            "--ticks" => o.ticks = num("--ticks", value("--ticks")) as u64,
            "--warmup" => o.warmup = num("--warmup", value("--warmup")) as u64,
            "--pace-ms" => o.pace_ms = num("--pace-ms", value("--pace-ms")),
            "--subs" => o.subs = num("--subs", value("--subs")) as usize,
            "--repeat" => o.repeat = (num("--repeat", value("--repeat")) as u32).max(1),
            "--out" => o.out = Some(PathBuf::from(value("--out"))),
            "--full" => o.full = true,
            p => o.project = PathBuf::from(p),
        }
    }
    o
}

/// Microseconds: median, mean, 95th percentile and largest.
fn stats(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let at = |q: f64| v[((n as f64 - 1.0) * q).round() as usize];
    let mean = v.iter().sum::<f64>() / n as f64;
    let r = |x: f64| (x * 100.0).round() / 100.0;
    json!({"median": r(at(0.5)), "mean": r(mean), "p95": r(at(0.95)), "max": r(v[n - 1])})
}

fn us(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

/// Waits for `due` in slices of at most 1 ms, as the game thread's `wait_until` does.
fn wait_until(due: Instant) {
    loop {
        let now = Instant::now();
        if now >= due {
            return;
        }
        std::thread::sleep((due - now).min(Duration::from_millis(1)));
    }
}

fn time_status(g: &Game) -> TimeStatus {
    let clock = g.sim().clock();
    TimeStatus {
        tick: clock.tick.0,
        t_s: clock.time(),
        pacing: PacingStatus::RealTime { speed: 1.0 },
        paused: false,
        halted: false,
        behind_ms: Some(0.0),
    }
}

/// The loop's work after each tick, part by part.
fn loop_parts(g: &mut Game, o: &Options) -> Value {
    let feed = Feed::new();
    let boxes: Vec<_> = (0..o.subs).map(|_| feed.subscribe()).collect();
    let mut extractor = Extractor::new();
    let start = Instant::now();
    let clock: Arc<dyn Fn() -> f64 + Send + Sync> =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let registry = Arc::new(g.registry_info().unwrap_or_default());
    let first = WorldSnapshot {
        version: 1,
        snapshot: g.snapshot().unwrap_or_else(|e| panic!("{e:#?}")),
        registry: registry.clone(),
        time: time_status(g),
        last_event: 0,
        published_at_ms: clock(),
    };
    let (publisher, reader) = publication(first, clock.clone());
    let mut cache = pocket_persist::SnapshotCache::new();
    let mut pushed = g.inbox().len();
    let (mut step, mut streams, mut present, mut snapshot, mut publish) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut events = 0usize;
    let mut event_bytes = 0usize;
    // Ticks after which each section's digest differed from the tick before's.
    let mut changed: BTreeMap<String, u64> = BTreeMap::new();
    // Digests only: holding the last snapshot would move its drop out of the publication.
    let mut last = reader.latest().snapshot.digests();
    let paced = o.pace_ms > 0.0;
    let mut due = Instant::now();
    for version in (2..).take(usize::try_from(o.ticks).unwrap_or(usize::MAX)) {
        if paced {
            due += Duration::from_secs_f64(o.pace_ms / 1000.0);
            wait_until(due);
        }
        let t0 = Instant::now();
        g.step().unwrap_or_else(|e| panic!("{e:#?}"));
        let t1 = Instant::now();
        pushed = pushed.min(g.inbox().len());
        let new: Vec<Arc<[u8]>> = g.inbox()[pushed..]
            .iter()
            .map(|ev| Arc::from(pocket_persist::pce::to_bytes(ev).unwrap_or_default()))
            .collect();
        pushed = g.inbox().len();
        let lines = g.take_log();
        let t2 = Instant::now();
        g.present(&mut extractor, &feed);
        let t3 = Instant::now();
        let snap = if o.full {
            g.snapshot()
        } else {
            cache.snapshot(g.world(), g.registry())
        }
        .unwrap_or_else(|e| panic!("{e:#?}"));
        let t4 = Instant::now();
        publisher.publish(WorldSnapshot {
            version,
            snapshot: snap,
            registry: registry.clone(),
            time: time_status(g),
            last_event: 0,
            published_at_ms: clock(),
        });
        let t5 = Instant::now();
        events += new.len();
        event_bytes += new.iter().map(|b| b.len()).sum::<usize>();
        std::hint::black_box((new, lines));
        let now = reader.latest().snapshot.digests();
        for (key, digest) in &now {
            let same = last.iter().any(|(k, d)| k == key && d == digest);
            *changed.entry(key.to_string()).or_default() += u64::from(!same);
        }
        last = now;
        // The editor viewports' mailboxes, as `/render` takes them (outside the timing).
        for m in boxes.iter().skip(1) {
            std::hint::black_box(m.take().encode());
        }
        step.push(us(t1 - t0));
        streams.push(us(t2 - t1));
        present.push(us(t3 - t2));
        snapshot.push(us(t4 - t3));
        publish.push(us(t5 - t4));
    }
    let after: Vec<f64> = (0..step.len())
        .map(|i| streams[i] + present[i] + snapshot[i] + publish[i])
        .collect();
    std::hint::black_box(reader.latest());
    json!({
        "ticks": o.ticks,
        "pace_ms": o.pace_ms,
        "subscribers": o.subs,
        "snapshot": if o.full { "full" } else { "cached" },
        "events_per_tick": events as f64 / o.ticks as f64,
        "event_bytes_per_tick": event_bytes as f64 / o.ticks as f64,
        "sections_changed_ticks": changed,
        "us": {
            "step": stats(step),
            "streams": stats(streams),
            "present": stats(present),
            "snapshot": stats(snapshot),
            "publish": stats(publish),
            "after_step": stats(after),
        },
    })
}

/// The snapshot's parts, back to back on the world as it is.
fn anatomy(g: &Game, o: &Options) -> Value {
    let world = g.world();
    let reg = g.registry().clone();
    let n = o.repeat;
    let time = |f: &mut dyn FnMut()| -> f64 {
        f();
        let t = Instant::now();
        for _ in 0..n {
            f();
        }
        us(t.elapsed()) / f64::from(n)
    };
    let snapshot = time(&mut || {
        std::hint::black_box(g.snapshot().ok());
    });
    let digests = time(&mut || {
        std::hint::black_box(pocket_persist::snapshot::section_digests(world, &reg).ok());
    });
    let checks = time(&mut || {
        std::hint::black_box(reg.check_classes(world).ok());
        std::hint::black_box(reg.check_rows(world).ok());
    });
    let snap = g.snapshot().unwrap_or_else(|e| panic!("{e:#?}"));
    let mut prefixed: Vec<Vec<u8>> = Vec::new();
    for s in snap.sections() {
        let mut buf = Vec::new();
        digest_prefix(&mut buf, s.key.kind, &s.key.name, s.version);
        buf.extend_from_slice(&s.bytes);
        prefixed.push(buf);
    }
    let hashing = time(&mut || {
        for b in &prefixed {
            std::hint::black_box(section_digest(b));
        }
    });
    let mut sections: Vec<(String, usize)> = snap
        .sections()
        .iter()
        .map(|s| (s.key.to_string(), s.bytes.len()))
        .collect();
    let bytes: usize = sections.iter().map(|s| s.1).sum();
    sections.sort_by_key(|s| std::cmp::Reverse(s.1));
    json!({
        "repeat": n,
        "us": {
            "snapshot": (snapshot * 100.0).round() / 100.0,
            "section_digests": (digests * 100.0).round() / 100.0,
            "registry_checks": (checks * 100.0).round() / 100.0,
            "hashing": (hashing * 100.0).round() / 100.0,
        },
        "registered_sections": reg.entries().len(),
        "sections": snap.sections().len(),
        "bytes": bytes,
        "largest": sections.iter().take(8).map(|(k, b)| json!([k, b])).collect::<Vec<_>>(),
        "entities": world.resource::<pocket_sim::EntityIndex>().len(),
    })
}

fn bench(o: &Options) -> Value {
    let dir = &o.project;
    let project = Project::load(dir).unwrap_or_else(|e| panic!("{}: {e:#?}", dir.display()));
    let seed = project.manifest.seed;
    let name = project.manifest.name.clone();
    let setup = Arc::new(project.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    let mut g: Game = GameBuilder::new(setup)
        .seed(seed)
        .project(dir.clone())
        .build()
        .unwrap_or_else(|e| panic!("{e:#?}"));
    g.run(o.warmup).unwrap_or_else(|e| panic!("{e:#?}"));
    let parts = loop_parts(&mut g, o);
    let anatomy = anatomy(&g, o);
    json!({"project": name, "warmup": o.warmup, "loop": parts, "anatomy": anatomy,
           "world_hash": g.world_hash().map(|h| h.to_string()).unwrap_or_default()})
}

fn main() {
    let o = options();
    let run = move || {
        let report = json!({"provisional": true, "run": bench(&o)});
        let text = serde_json::to_string_pretty(&report).unwrap_or_default();
        println!("{text}");
        if let Some(path) = &o.out {
            std::fs::write(path, text + "\n").unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    };
    // Scripts need the script host's stack (script-sandbox.md 4.3).
    std::thread::Builder::new()
        .stack_size(pocket_runtime::GAME_STACK_BYTES)
        .spawn(run)
        .expect("a thread")
        .join()
        .expect("the bench ran");
}
