//! The level-scale scene of `physics_bench level` (`probe::level`; docs/bench/physics.md, "Static
//! geometry and the cache") as a WebAssembly module with no imports, for timing the physics cache in
//! a JavaScript engine: each export does one operation and `tests/web/level.mjs` times the calls.
//! An operation keeps its result until `release` frees it, outside the timed call.
//!
//! `cargo build -p amoris-physics --example level_web --target wasm32-unknown-unknown --release`,
//! then `node crates/amoris-physics/tests/web/level.mjs <the .wasm> CELLS [--side 10] [--warm 150]
//! [--rounds 5] [--check 30]`, which prints one JSON line like the native bench's.

use std::cell::RefCell;

use amoris_physics::PhysicsCache;
use amoris_physics::probe::{Snap, fork, fresh, ledger, level};
use amoris_sim::persisted::{PersistedCache, Staged};
use amoris_sim::{NoHooks, Sim};

#[derive(Default)]
struct State {
    sim: Option<Sim>,
    snap: Option<Snap>,
    copy: Option<Sim>,
    forked: Option<Sim>,
    staged: Option<Box<dyn Staged>>,
    bytes: Vec<u8>,
    text: String,
}

/// `release`'s bits: the cache's bytes, the decoded cache, the snapshot, the restored copy, the
/// fork.
const BYTES: u32 = 1;
const STAGED: u32 = 2;
const SNAP: u32 = 4;
const COPY: u32 = 8;
const FORKED: u32 = 16;

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn step(sim: &mut Sim) -> bool {
    sim.step(&mut NoHooks).is_ok_and(|r| r.errors.is_empty())
}

/// Makes the scene: a terrain of `2 * cells^2` triangles and `4 * (2 * side + 1)^2` bodies.
#[unsafe(no_mangle)]
pub extern "C" fn setup(cells: u32, side: u32) -> u32 {
    let side = i32::try_from(side).unwrap_or(10);
    with(|s| s.sim = Some(level::level_world(cells, side)));
    1
}

/// One tick of the scene; 0 if it failed or reported an error.
#[unsafe(no_mangle)]
pub extern "C" fn tick() -> u32 {
    with(|s| s.sim.as_mut().is_some_and(step).into())
}

/// The terrain's spec hash, as `physics.step` computes it every tick; its low 32 bits.
#[unsafe(no_mangle)]
pub extern "C" fn terrain_spec_hash() -> u32 {
    with(|s| {
        s.sim.as_ref().map_or(0, |sim| {
            u32::try_from(level::terrain_spec_hash(sim) & 0xffff_ffff).unwrap_or(0)
        })
    })
}

/// Frees the results `what` names (the bits above).
#[unsafe(no_mangle)]
pub extern "C" fn release(what: u32) {
    with(|s| {
        if what & BYTES != 0 {
            s.bytes = Vec::new();
        }
        if what & STAGED != 0 {
            s.staged = None;
        }
        if what & SNAP != 0 {
            s.snap = None;
        }
        if what & COPY != 0 {
            s.copy = None;
        }
        if what & FORKED != 0 {
            s.forked = None;
        }
    });
}

/// The physics cache's encode, into a new buffer as a snapshot's; its length.
#[unsafe(no_mangle)]
pub extern "C" fn cache_encode() -> u32 {
    with(|s| {
        let Some(sim) = &s.sim else { return 0 };
        let mut out = Vec::new();
        if PhysicsCache::encode(sim.world(), &mut out).is_err() {
            return 0;
        }
        s.bytes = out;
        u32::try_from(s.bytes.len()).unwrap_or(u32::MAX)
    })
}

/// The physics cache's decode of the last encode, kept; 0 if it failed.
#[unsafe(no_mangle)]
pub extern "C" fn cache_decode() -> u32 {
    with(|s| {
        s.staged = PhysicsCache::decode(&s.bytes).ok();
        s.staged.is_some().into()
    })
}

/// The stand-in snapshot (`probe::Ledger`), kept for `restore`; its total length.
#[unsafe(no_mangle)]
pub extern "C" fn snapshot() -> u32 {
    with(|s| {
        let Some(sim) = &s.sim else { return 0 };
        let Ok(snap) = ledger().snapshot(sim.world()) else {
            return 0;
        };
        let n = snap.sections.iter().map(|x| x.1.len()).sum::<usize>();
        s.snap = Some(snap);
        u32::try_from(n).unwrap_or(u32::MAX)
    })
}

/// The length of one section of the kept snapshot.
#[unsafe(no_mangle)]
pub extern "C" fn section_len(which: u32) -> u32 {
    let name = if which == 0 {
        "physics.rapier"
    } else {
        "Collider"
    };
    with(|s| {
        s.snap
            .as_ref()
            .and_then(|x| x.section(name))
            .map_or(0, |b| u32::try_from(b.len()).unwrap_or(u32::MAX))
    })
}

/// Restores the kept snapshot into a fresh world, kept for `copy_same`; 0 if it failed.
#[unsafe(no_mangle)]
pub extern "C" fn restore() -> u32 {
    with(|s| {
        let Some(snap) = &s.snap else { return 0 };
        let mut copy = fresh();
        if ledger().restore(&mut copy, snap).is_err() {
            return 0;
        }
        s.copy = Some(copy);
        1
    })
}

/// 1 if the last restored world's snapshot equals the kept one.
#[unsafe(no_mangle)]
pub extern "C" fn copy_same() -> u32 {
    with(|s| match (&s.copy, &s.snap) {
        (Some(c), Some(snap)) => ledger()
            .snapshot(c.world())
            .is_ok_and(|x| x == *snap)
            .into(),
        _ => 0,
    })
}

/// A fork of the scene, kept.
#[unsafe(no_mangle)]
pub extern "C" fn fork_once() -> u32 {
    with(|s| {
        let Some(sim) = &s.sim else { return 0 };
        s.forked = fork(&ledger(), sim, fresh).ok();
        s.forked.is_some().into()
    })
}

/// A fork stepped `ticks` ticks beside its source: 1 if every tick's hash matched.
#[unsafe(no_mangle)]
pub extern "C" fn check(ticks: u32) -> u32 {
    with(|s| {
        let Some(sim) = s.sim.as_mut() else { return 0 };
        let l = ledger();
        let Ok(mut f) = fork(&l, sim, fresh) else {
            return 0;
        };
        let hash = |x: &Sim| l.snapshot(x.world()).map(|s| s.hash()).ok();
        let mut same = hash(&f) == hash(sim);
        for _ in 0..ticks {
            same &= step(sim) && step(&mut f) && hash(&f) == hash(sim);
        }
        same.into()
    })
}

/// Drops the scene (keeping the snapshot), so that a `restore` after it is cold.
#[unsafe(no_mangle)]
pub extern "C" fn drop_scene() {
    with(|s| s.sim = None);
}

/// Writes the cache's identity and the kept snapshot's hash as text; its length (`text_ptr`).
#[unsafe(no_mangle)]
pub extern "C" fn describe() -> u32 {
    with(|s| {
        let hash = s.snap.as_ref().map_or(0, |x| x.hash());
        s.text = format!("{}\n{hash:016x}", PhysicsCache::identity());
        u32::try_from(s.text.len()).unwrap_or(0)
    })
}

/// The address of the text `describe` wrote.
#[unsafe(no_mangle)]
pub extern "C" fn text_ptr() -> *const u8 {
    with(|s| s.text.as_ptr())
}
