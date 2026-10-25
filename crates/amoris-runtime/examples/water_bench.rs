//! The water's costs on the CPU (docs/spec/water.md 12 and 13; docs/bench/liquids.md): blockcraft
//! with its broken bank (amoris-app's `blockcraft_water/breach.rs`), stepped back to back, and
//! after each tick, timed one by one:
//!
//! - **solver**: `physics.water` inside the tick (a step hook around the system);
//! - **snapshot**: the world's snapshot as the game thread takes it, through amoris-persist's
//!   `SnapshotCache`, and the size of the `WaterRegion` section;
//! - **feed**: the render feed's extraction for one subscriber, and the frame's encoded bytes.
//!
//! It prints the pit's water and when the region sleeps again after the break, then the medians
//! over windows of ticks: every tile awake (the field liquids converted at tick 1 wake the whole
//! region until it sleeps), asleep (before the break), the flood, the minutes after it, and asleep
//! again, with the tiles stepped a tick.
//!
//! `cargo run --release -p amoris-runtime --features transpile --example water_bench -- \
//!  [--ticks 1200] [project]` (the project defaults to samples/blockcraft).

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick
#![allow(clippy::cast_precision_loss)] // counts and microseconds into f64 for reporting

#[path = "../../amoris-app/examples/blockcraft_water/breach.rs"]
mod breach;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use amoris_assets::Feed;
use amoris_persist::SnapshotCache;
use amoris_physics::{WaterRegion, WaterSteps};
use amoris_runtime::{Extractor, Game, Project};
use amoris_sim::{StepHooks, SystemKey, TickPhase};

/// Times `physics.water`.
#[derive(Default)]
struct Hooks {
    start: Option<Instant>,
    ms: f64,
}

impl StepHooks for Hooks {
    fn system(&mut self, key: &SystemKey, begin: bool) {
        if key.to_string() != "physics.water" {
            return;
        }
        if begin {
            self.start = Some(Instant::now());
        } else if let Some(s) = self.start.take() {
            self.ms = s.elapsed().as_secs_f64() * 1e3;
        }
    }

    fn phase(&mut self, _phase: TickPhase, _begin: bool) {}
}

/// One tick's measurements.
struct Row {
    tick: u64,
    tiles: usize,
    /// Tiles awake after the tick, and the water in the pit, m^3.
    awake: usize,
    pit: f64,
    solver: f64,
    snapshot: f64,
    section: usize,
    feed: f64,
    bytes: usize,
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut ticks = 1200u64;
    let mut project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/blockcraft");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ticks" => ticks = it.next().and_then(|s| s.parse().ok()).unwrap_or(ticks),
            other => project = PathBuf::from(other),
        }
    }
    let (root, site) = breach::prepare(&project);
    println!("breach at cells {:?}, world {:?}", site.cell, site.world);
    let p = Project::load(&root).unwrap_or_else(|e| panic!("{e:#?}"));
    let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    let mut game = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
    let feed = Feed::new();
    let mailbox = feed.subscribe();
    let mut extractor = Extractor::new();
    game.present(&mut extractor, &feed);
    let _ = mailbox.take();
    let mut cache = SnapshotCache::new();
    let mut rows = Vec::new();
    for _ in 0..ticks {
        let mut hooks = Hooks::default();
        game.step_with(&mut hooks)
            .unwrap_or_else(|e| panic!("{e:#?}"));
        let tick = game.tick().0;
        let tiles = game.world().get_resource::<WaterSteps>().map_or(0, |s| {
            s.regions
                .values()
                .map(|m| m.iter().filter(|&&t| t == tick).count())
                .sum()
        });
        let (awake, pit) = {
            let mut q = game.world().try_query::<&WaterRegion>().expect("a region");
            let r = q.single(game.world()).expect("one region");
            let awake = r.tiles.iter().filter(|t| !t.asleep()).count();
            let [cx, cz] = site.cell.map(|c| usize::try_from(c).unwrap_or(0));
            let mut pit = 0.0;
            if r.well_formed() {
                for k in 2 * cz..2 * (cz + 6) {
                    for i in 2 * (cx + 2)..2 * (cx + 10) {
                        pit += r.depth_at(i, k) * r.cell_m * r.cell_m;
                    }
                }
            }
            (awake, pit)
        };
        let t = Instant::now();
        let snap = cache
            .snapshot(game.world(), game.registry())
            .unwrap_or_else(|e| panic!("{e:#?}"));
        let snapshot = t.elapsed().as_secs_f64() * 1e3;
        let section = snap
            .sections()
            .iter()
            .find(|s| s.key.name == "WaterRegion")
            .map_or(0, |s| s.bytes.len());
        let t = Instant::now();
        game.present(&mut extractor, &feed);
        let feed_ms = t.elapsed().as_secs_f64() * 1e3;
        let bytes = mailbox.take().encode().len();
        rows.push(Row {
            tick,
            tiles,
            awake,
            pit,
            solver: hooks.ms,
            snapshot,
            section,
            feed: feed_ms,
            bytes,
        });
    }
    let mut q = game.world().try_query::<&WaterRegion>().expect("a region");
    let r = q.single(game.world()).expect("one region");
    println!(
        "region {:?} columns of {} m, {} tiles; water {:.1} m3 at the end",
        r.size,
        r.cell_m,
        r.tiles.len(),
        r.volume()
    );
    // The flood (the pit's water) and the sleep after it.
    let at = |dt: u64| rows.iter().find(|r| r.tick == breach::BREAK + dt);
    let full = rows.last().map_or(0.0, |r| r.pit);
    let filled = rows
        .iter()
        .find(|r| r.tick > breach::BREAK && r.pit >= 0.99 * full)
        .map(|r| (r.tick - breach::BREAK) as f64 / 60.0);
    let slept = rows
        .iter()
        .rev()
        .take_while(|r| r.awake == 0)
        .last()
        .filter(|r| r.tick > breach::BREAK)
        .map(|r| (r.tick - breach::BREAK) as f64 / 60.0);
    println!(
        "the pit: {:.1} m3 at 5 s, {:.1} at 10 s, {:.1} at 15 s, {full:.1} at the end; 99% at \
         {filled:?} s after the break",
        at(300).map_or(0.0, |r| r.pit),
        at(600).map_or(0.0, |r| r.pit),
        at(900).map_or(0.0, |r| r.pit),
    );
    println!(
        "tiles awake: {:?} at 10 s, {:?} at 30 s, {:?} at 60 s; all asleep from {slept:?} s \
         after the break",
        at(600).map(|r| r.awake),
        at(1800).map(|r| r.awake),
        at(3600).map(|r| r.awake),
    );
    println!(
        "window | ticks | tiles stepped (median) | solver ms | snapshot ms | WaterRegion section \
         bytes | feed ms | frame bytes"
    );
    let windows = [
        ("all awake", 2u64, 60u64),
        ("asleep", 70, breach::BREAK),
        ("flood", breach::BREAK + 1, breach::BREAK + 600),
        ("after", breach::BREAK + 601, breach::BREAK + 1800),
        ("later", breach::BREAK + 1801, breach::BREAK + 5400),
        ("latest", breach::BREAK + 5401, ticks),
    ];
    for (name, lo, hi) in windows {
        let w: Vec<&Row> = rows
            .iter()
            .filter(|r| (lo..=hi).contains(&r.tick))
            .collect();
        if w.is_empty() {
            continue;
        }
        let m = |f: fn(&Row) -> f64| median(w.iter().map(|r| f(r)).collect());
        println!(
            "{name} | {lo}-{hi} | {:.0} | {:.3} | {:.3} | {:.0} | {:.3} | {:.0}",
            m(|r| r.tiles as f64),
            m(|r| r.solver),
            m(|r| r.snapshot),
            m(|r| r.section as f64),
            m(|r| r.feed),
            m(|r| r.bytes as f64),
        );
    }
    let _ = std::fs::remove_dir_all(root);
}
