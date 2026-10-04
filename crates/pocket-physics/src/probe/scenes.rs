//! The scenes the checks run and the web tests (checks.md 7.2, `tests`):
//!
//! - `physics.chain`: the sailing scene (the Sloop under a gusty breeze on the island sea among six
//!   crates, a scripted crew setting the sail and the rudder) run for [`CHAIN_TICKS`] ticks, its
//!   hash at every tick folded into a chain that must equal the committed golden one
//!   (`golden.txt`), natively in debug and release and in WebAssembly.
//! - `physics.fork`: a fork at every tick of the first 600, each stepped 10 ticks, matches the
//!   original's hashes, and stepping only a fork leaves the original's state unchanged.
//! - `physics.bvh_log2`: parry's BVH rounding of `log2` (numeric.md 5, 10 item 8) agrees between
//!   the platform's `log2f` and libm's for every leaf count from 1 to 1,000,000.

use bevy_ecs::prelude::{Query, Res};
use pocket_sim::order::by_id_mut;
use pocket_sim::{
    EntityId, Name, NoHooks, RunCondition, Sim, SimClock, SimConfig, TickPhase, TickRate, entity,
};

use super::{Ledger, fork, ledger};
use crate::boat::Boat;
use crate::sailing;
use crate::sea::Wind;

/// Ticks of the hash chain.
pub const CHAIN_TICKS: u64 = 1200;

/// The crew of the sailing scene: the sail up and eased at tick 30, bear away at 300, head up with
/// the sheet in at 700, bear away and ease at 1000, furl at 1100.
pub fn crew(clock: Res<SimClock>, mut boats: Query<(&EntityId, &mut Boat)>) {
    let t = clock.tick.0;
    for (_, mut b) in by_id_mut(&mut boats) {
        match t {
            30 => {
                b.hoist = 1.0;
                b.sheet = 0.55;
            }
            300 => {
                b.rudder = 0.35;
                b.sheet = 0.8;
            }
            420 | 800 => b.rudder = 0.0,
            700 => {
                b.rudder = -0.5;
                b.sheet = 0.4;
            }
            1000 => {
                b.rudder = 0.6;
                b.sheet = 0.9;
            }
            1100 => b.hoist = 0.0,
            _ => {}
        }
    }
}

/// A world with physics and the crew and no entities: what a fork restores into.
pub fn fresh() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 1,
    })
    .expect("a valid configuration");
    crate::plugin(&mut sim).expect("physics installs");
    sim.add_system("probe.crew", TickPhase::Update, RunCondition::Always, crew)
        .expect("the crew's key is free");
    sim
}

/// Where the crates float: two on the Sloop's track, two touching each other, two further off.
const CRATES: [[f64; 3]; 6] = [
    [0.6, 0.0, -9.0],
    [1.3, 0.0, -9.4],
    [5.4, 0.0, -21.2],
    [6.0, 0.0, -20.6],
    [-3.0, 0.0, 4.0],
    [9.4, 0.0, -41.5],
];

/// The sailing scene: the island sea, a gusty westerly of 6 m/s, the Sloop at the origin heading
/// north with the wind on its port beam, and six crates.
pub fn sailing_world() -> Sim {
    let mut sim = fresh();
    let mut b = sim.boundary();
    let name = |n: &str| Name::new(n).expect("a short name");
    b.spawn((name("Sea"), sailing::island_sea()))
        .expect("ids remain");
    let wind = Wind {
        gust: 0.15,
        ..sailing::breeze(270.0, 6.0)
    };
    b.spawn((name("Breeze"), wind)).expect("ids remain");
    b.spawn((name("Sloop"), sailing::sloop([0.0; 3], 0.0)))
        .expect("ids remain");
    for (i, c) in CRATES.iter().enumerate() {
        let yaw = 21.0 * crate::geom::f64_of(i);
        b.spawn((name(&format!("Crate{i}")), sailing::crate_box(*c, yaw)))
            .expect("ids remain");
    }
    sim
}

/// Master's `sailboat` task (tools/scripts/agent_eval.py, `sailboat_check`), with the contract's
/// wind convention: the sea at 0, a steady westerly of 6 (toward +x), and the Sloop near the
/// origin with its bow toward +x and its sail set (`hoisted`: already up, or hoisted from tick 1
/// at `HOIST_RATE`). Returns the world and the Sloop's id.
pub fn sailboat_world(hoisted: bool) -> (Sim, EntityId) {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 1,
    })
    .expect("a valid configuration");
    crate::plugin(&mut sim).expect("physics installs");
    let mut b = sim.boundary();
    b.spawn((Name::new("Sea").expect("short"), sailing::island_sea()))
        .expect("ids remain");
    b.spawn((
        Name::new("Breeze").expect("short"),
        sailing::breeze(270.0, 6.0),
    ))
    .expect("ids remain");
    let mut sloop = sailing::sloop([0.0; 3], 90.0);
    sloop.6 = if hoisted {
        Boat::sail_set()
    } else {
        Boat {
            hoist: 1.0,
            ..Boat::default()
        }
    };
    let id = b
        .spawn((Name::new("Sloop").expect("short"), sloop))
        .expect("ids remain");
    (sim, id)
}

/// The Sloop's position in a world made by [`sailing_world`] (its third entity).
pub fn boat_position(sim: &Sim) -> crate::geom::V3 {
    let id = EntityId::new(3).expect("3 is an id");
    entity::require(sim.world(), id)
        .ok()
        .and_then(|e| sim.world().get::<crate::body::Transform>(e))
        .map_or(crate::geom::ZERO, |t| t.position)
}

/// The sailing scene's hash at tick 0 and after each of `ticks` ticks.
pub fn chain(ticks: u64) -> Result<Vec<u64>, String> {
    let l = ledger();
    let mut sim = sailing_world();
    let mut out = vec![l.snapshot(sim.world())?.hash()];
    for _ in 0..ticks {
        sim.step(&mut NoHooks).map_err(|p| p.message)?;
        out.push(l.snapshot(sim.world())?.hash());
    }
    Ok(out)
}

fn fold(hashes: &[u64]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for x in hashes {
        h = pocket_sim::rng::fnv1a64_extend(h, &x.to_le_bytes());
    }
    h
}

/// The committed chain: `chain <hex> ticks <n>`, rewritten by
/// `POCKET_BLESS=1 cargo test -p pocket-physics --test web` with a deliberate change of results.
pub const GOLDEN: &str = include_str!("golden.txt");

/// The golden line for a chain.
pub fn golden_line(hashes: &[u64]) -> String {
    format!(
        "chain {:016x} ticks {}\n",
        fold(hashes),
        hashes.len().saturating_sub(1)
    )
}

/// `physics.chain`.
pub fn check_chain() -> Result<(), String> {
    let line = golden_line(&chain(CHAIN_TICKS)?);
    if line.trim() == GOLDEN.trim() {
        Ok(())
    } else {
        Err(format!(
            "the sailing scene's chain is `{}`, the golden one `{}`",
            line.trim(),
            GOLDEN.trim()
        ))
    }
}

/// The report's hash lines: `tick hash` for every tick of the chain.
pub fn chain_report() -> String {
    match chain(CHAIN_TICKS) {
        Ok(h) => h
            .iter()
            .enumerate()
            .map(|(t, x)| format!("{t} {x:016x}\n"))
            .collect(),
        Err(e) => format!("FAIL chain: {e}\n"),
    }
}

fn step(sim: &mut Sim) -> Result<(), String> {
    sim.step(&mut NoHooks).map(drop).map_err(|p| p.message)
}

/// `physics.fork`: a fork at every tick 0 to 599 continues 10 ticks with the original's hashes; a
/// fork stepped alone, its boat steered away, leaves the original's state unchanged.
pub fn check_forks() -> Result<(), String> {
    let (l, forks, run) = (ledger(), 600u64, 10u64);
    let mut reference = chain(forks + run)?.into_iter();
    let expected: Vec<u64> = reference.by_ref().collect();
    let mut sim = sailing_world();
    for t in 0..forks {
        let mut f = fork(&l, &sim, fresh)?;
        let tick = usize::try_from(t).map_err(|e| e.to_string())?;
        if l.snapshot(f.world())?.hash() != expected[tick] {
            return Err(format!("the fork at tick {t} differs from its source"));
        }
        for k in 1..=run {
            step(&mut f)?;
            let at = tick + usize::try_from(k).map_err(|e| e.to_string())?;
            if l.snapshot(f.world())?.hash() != expected[at] {
                return Err(format!("the fork made at tick {t} diverged at tick {at}"));
            }
        }
        step(&mut sim)?;
    }
    independence(&l, &mut sim)
}

/// Stepping only a fork, with its boat steered hard over, leaves the original as it was.
fn independence(l: &Ledger, sim: &mut Sim) -> Result<(), String> {
    let before = l.snapshot(sim.world())?;
    let mut f = fork(l, sim, fresh)?;
    let sloop = f
        .world()
        .resource::<pocket_sim::EntityIndex>()
        .ids()
        .nth(2)
        .ok_or("no sloop")?;
    let e = entity::require(f.world(), sloop).map_err(|p| p.message)?;
    if let Some(mut b) = f.world_mut().get_mut::<Boat>(e) {
        b.rudder = -1.0;
    }
    for _ in 0..120 {
        step(&mut f)?;
    }
    if l.snapshot(sim.world())? != before {
        return Err("stepping a fork changed the original".into());
    }
    if l.snapshot(f.world())? == before {
        return Err("the fork did not move".into());
    }
    Ok(())
}

/// parry 0.31.1's `Bvh::optimization_config` subtree count (`src/partitioning/bvh/
/// bvh_optimize.rs`, lines 13 to 30), with the platform's `log2f` or libm's.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::disallowed_methods
)] // parry's own expression, copied as it is
fn subtree_count(num_leaves: u32, log2: fn(f32) -> f32) -> usize {
    let num_optimized_leaves = (num_leaves * 5).div_ceil(100);
    let num_leaves_sqrt = (num_leaves as f32).sqrt();
    let target_root_node_count = num_leaves_sqrt.ceil();
    let target_subtree_leaf_count = (num_leaves_sqrt * 4.0).ceil();
    let root_refinement_cost = target_root_node_count * log2(target_root_node_count)
        / (target_subtree_leaf_count * log2(target_subtree_leaf_count));
    (num_optimized_leaves as f32 / target_subtree_leaf_count - root_refinement_cost)
        .round()
        .max(0.0) as usize
}

/// The platform's `log2f`, which parry calls natively.
#[allow(clippy::disallowed_methods)] // the platform's function is what this check compares
fn platform_log2(x: f32) -> f32 {
    x.log2()
}

/// `physics.bvh_log2`.
pub fn check_bvh_log2() -> Result<(), String> {
    for n in 1..=1_000_000u32 {
        let (a, b) = (
            subtree_count(n, platform_log2),
            subtree_count(n, libm::log2f),
        );
        if a != b {
            return Err(format!(
                "{n} leaves: {a} subtrees with the platform's log2, {b} with libm's"
            ));
        }
    }
    Ok(())
}
