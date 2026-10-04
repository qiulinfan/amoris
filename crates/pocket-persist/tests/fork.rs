//! A fork made at every tick of a small world continues identically (charter 3.3; persistence.md
//! 7; checks.md 8.3): at each boundary F of a reference run, B = fork(A) is identical to A, and B
//! run on with the same inputs gives the reference's hash at every later tick.

mod common;

use common::{Game, game, inputs, run};
use pocket_persist::{fork, snapshot, world_hash};
use pocket_sim::{NoHooks, WorldSeed};

const TICKS: u64 = 300;

fn hashes_to(g: &mut Game, from: u64, ins: &[common::Input]) -> Vec<pocket_persist::WorldHash> {
    let mut out = Vec::new();
    while g.sim.clock().tick.0 < TICKS {
        let t = g.sim.clock().tick.0 + 1;
        for i in ins.iter().filter(|i| i.tick == t) {
            let _ = common::apply(&mut g.sim, i.name, &i.params);
        }
        g.sim.step(&mut NoHooks).unwrap();
        out.push(world_hash(g.sim.world(), &g.reg).unwrap());
    }
    assert_eq!(out.len() as u64, TICKS - from);
    out
}

#[test]
fn a_fork_at_every_tick_continues_identically() {
    let ins = inputs();
    let mut reference = game(21);
    let all = hashes_to(&mut reference, 0, &ins);
    let mut a = game(21);
    for f in 0..TICKS {
        run(&mut a, None, &ins, f);
        let hash_a = world_hash(a.sim.world(), &a.reg).unwrap();
        // The fork is a new world: its own schedule (from the game's builder, another seed, which
        // the restore replaces) and the source's persisted state.
        let mut b = game(999);
        let forked = fork(a.sim.world(), &a.reg, || {
            std::mem::replace(b.sim.world_mut(), bevy_ecs::world::World::new())
        })
        .unwrap();
        *b.sim.world_mut() = forked;
        assert_eq!(
            world_hash(b.sim.world(), &b.reg).unwrap(),
            hash_a,
            "identical at the fork {f}"
        );
        assert_eq!(
            snapshot(b.sim.world(), &b.reg).unwrap().to_bytes(),
            snapshot(a.sim.world(), &a.reg).unwrap().to_bytes()
        );
        assert_eq!(b.sim.world().resource::<WorldSeed>().0, 21);
        let rest = hashes_to(&mut b, f, &ins);
        let expected = &all[f as usize..];
        if let Some(i) = rest.iter().zip(expected).position(|(x, y)| x != y) {
            panic!("the fork at {f} diverged at tick {}", f + 1 + i as u64);
        }
    }
}
