//! Flowing liquids (docs/spec/liquids.md 4): `physics.fluids` steps the block game's flow over each
//! volume's pending cells, as a cellular automaton in integers. Each liquid steps on its own period;
//! a step computes every new state from the volume as it stood and then writes them all, so the
//! order cells are visited in changes nothing. The pending cells are world state (`Voxels.fluid`),
//! so a restore or a fork continues the same flow.

use std::collections::{BTreeMap, BTreeSet};

use amoris_sim::order::by_id_mut;
use amoris_sim::voxels::{BlockShape, CHUNK, Cell, FALLING, Flow, LEVEL, Voxels};
use amoris_sim::{EntityId, SimClock};
use bevy_ecs::prelude::{Query, Res};

/// The most cells one flow step takes; the rest wait for the next period (liquids.md 4.2).
pub const MAX_STEP_CELLS: usize = 65_536;

const SIDES: [Cell; 4] = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]];
const AROUND: [Cell; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

fn add(a: Cell, b: Cell) -> Cell {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn is_source(state: u8) -> bool {
    state & (LEVEL | FALLING) == 0
}

/// The volume as one flow step reads it, with the box of its chunks: a liquid never leaves it.
struct Grid<'a> {
    v: &'a Voxels,
    lo: Cell,
    hi: Cell,
}

impl<'a> Grid<'a> {
    fn new(v: &'a Voxels) -> Grid<'a> {
        let mut lo = [i32::MAX; 3];
        let mut hi = [i32::MIN; 3];
        for c in &v.chunks {
            for a in 0..3 {
                lo[a] = lo[a].min(c.key[a] * CHUNK);
                hi[a] = hi[a].max(c.key[a] * CHUNK + CHUNK - 1);
            }
        }
        Grid { v, lo, hi }
    }

    fn inside(&self, c: Cell) -> bool {
        (0..3).all(|a| self.lo[a] <= c[a] && c[a] <= self.hi[a])
    }

    /// Whether liquid `f` may take the cell: air, a plant, or `f` itself flowing, inside the box.
    fn open(&self, c: Cell, f: u8) -> bool {
        let (b, s) = self.v.cell(c);
        self.inside(c)
            && (b == 0 || self.v.shape(b) == BlockShape::Cross || (b == f && !is_source(s)))
    }

    /// Whether liquid at `c` falls rather than spreading: the cell below is open to it.
    fn falls(&self, c: Cell, f: u8) -> bool {
        self.open(add(c, [0, -1, 0]), f)
    }

    /// The directions liquid `f` at `c` spreads in: those with the nearest way down within
    /// `seek` cells over the ground, or all open ones when there is none (liquids.md 4.2).
    fn spreads(&self, c: Cell, f: u8, seek: u8, cache: &mut BTreeMap<Cell, u8>) -> u8 {
        if let Some(&m) = cache.get(&c) {
            return m;
        }
        let mut best = u32::MAX;
        let mut mask = 0u8;
        let mut open = 0u8;
        for (i, d) in SIDES.iter().enumerate() {
            let n = add(c, *d);
            if !self.open(n, f) {
                continue;
            }
            open |= 1 << i;
            if let Some(dist) = self.down_within(n, *d, f, u32::from(seek)) {
                match dist.cmp(&best) {
                    std::cmp::Ordering::Less => {
                        best = dist;
                        mask = 1 << i;
                    }
                    std::cmp::Ordering::Equal => mask |= 1 << i,
                    std::cmp::Ordering::Greater => {}
                }
            }
        }
        let m = if mask == 0 { open } else { mask };
        cache.insert(c, m);
        m
    }

    /// The fewest cells, from `start` (one step from the source cell, entered along `from`), to a
    /// cell liquid `f` could fall from, walking open cells over the ground; `None` past `seek`.
    fn down_within(&self, start: Cell, from: Cell, f: u8, seek: u32) -> Option<u32> {
        let back = from.map(|x| -x);
        let mut frontier = vec![(start, back)];
        let mut seen = BTreeSet::from([start]);
        for dist in 1..=seek {
            let mut next = Vec::new();
            for (c, came_from) in frontier {
                if self.falls(c, f) {
                    return Some(dist);
                }
                for d in SIDES {
                    if d == came_from {
                        continue;
                    }
                    let n = add(c, d);
                    if self.open(n, f) && seen.insert(n) {
                        next.push((n, d.map(|x| -x)));
                    }
                }
            }
            frontier = next;
        }
        None
    }

    /// The state liquid `f` gives cell `c` at this step, or `None` when it gives none.
    fn liquid(&self, c: Cell, f: u8, flow: Flow, cache: &mut BTreeMap<Cell, u8>) -> Option<u8> {
        let (b, s) = self.v.cell(c);
        if b == f && is_source(s) {
            return Some(0);
        }
        if self.v.get(add(c, [0, 1, 0])) == f {
            return Some(FALLING);
        }
        if flow.renews {
            let sources = SIDES
                .iter()
                .filter(|d| {
                    let (nb, ns) = self.v.cell(add(c, **d));
                    nb == f && is_source(ns)
                })
                .count();
            let (bb, bs) = self.v.cell(add(c, [0, -1, 0]));
            let ground = self.v.solid(bb) || (bb == f && is_source(bs));
            if sources >= 2 && ground {
                return Some(0);
            }
        }
        let mut level: Option<u8> = None;
        for (i, d) in SIDES.iter().enumerate() {
            let n = add(c, *d);
            let (nb, ns) = self.v.cell(n);
            if nb != f {
                continue;
            }
            let eff = if ns & FALLING != 0 { 0 } else { ns & LEVEL };
            if eff >= flow.spread || self.falls(n, f) {
                continue;
            }
            // From n, c lies in the opposite direction to d.
            let toward = i ^ 1;
            if self.spreads(n, f, flow.seek, cache) & (1 << toward) == 0 {
                continue;
            }
            level = Some(level.map_or(eff + 1, |l| l.min(eff + 1)));
        }
        level
    }
}

/// Whether the volume has flow work at `tick`: edits to take, or pending cells of a liquid due.
pub fn due(v: &Voxels, tick: u64) -> bool {
    let mut flows = (0..v.palette.len())
        .filter_map(|i| u8::try_from(i).ok())
        .filter_map(|b| v.flow(b))
        .peekable();
    if flows.peek().is_none() {
        return false;
    }
    let stepping = flows.any(|f| tick.is_multiple_of(u64::from(f.period_ticks)));
    v.chunks.iter().any(|c| c.edited > v.fluid.seen) || (stepping && !v.fluid.pending.is_empty())
}

/// One volume's work this tick: take the edits since `seen`, then step the liquids whose period
/// falls on `tick`. Returns whether the volume changed.
pub fn advance(v: &mut Voxels, tick: u64) -> bool {
    let flowing: Vec<(u8, Flow)> = (0..v.palette.len())
        .filter_map(|i| u8::try_from(i).ok())
        .filter_map(|b| v.flow(b).map(|f| (b, f)))
        .collect();
    if flowing.is_empty() {
        return false;
    }
    let edited = v.chunks.iter().any(|c| c.edited > v.fluid.seen);
    let due: Vec<(u8, Flow)> = flowing
        .iter()
        .copied()
        .filter(|(_, f)| tick.is_multiple_of(u64::from(f.period_ticks)))
        .collect();
    if !edited && (v.fluid.pending.is_empty() || due.is_empty()) {
        return false;
    }
    let mut pending: BTreeSet<Cell> = v.fluid.pending.iter().copied().collect();
    if edited {
        take_edits(v, &flowing, &mut pending);
    }
    let mut writes: Vec<(Cell, u8, u8)> = Vec::new();
    let mut rest: BTreeSet<Cell> = BTreeSet::new();
    if !due.is_empty() {
        let grid = Grid::new(v);
        let mut caches: Vec<BTreeMap<Cell, u8>> = vec![BTreeMap::new(); due.len()];
        for (k, &c) in pending.iter().enumerate() {
            if k >= MAX_STEP_CELLS {
                rest.insert(c);
                continue;
            }
            let (b, s) = v.cell(c);
            let held = flowing.iter().find(|(f, _)| *f == b);
            if let Some((f, _)) = held
                && !due.iter().any(|(d, _)| d == f)
            {
                // Another liquid's cell, not due this tick.
                rest.insert(c);
                continue;
            }
            let open = b == 0 || v.shape(b) == BlockShape::Cross || held.is_some();
            if !open || !grid.inside(c) {
                continue;
            }
            let mut new = None;
            for (j, (f, flow)) in due.iter().enumerate() {
                if b != 0 && b != *f && held.is_some() {
                    continue;
                }
                if let Some(state) = grid.liquid(c, *f, *flow, &mut caches[j]) {
                    new = Some((*f, state));
                    break;
                }
            }
            match new {
                Some((f, state)) if (f, state) != (b, s) => writes.push((c, f, state)),
                None if held.is_some() => writes.push((c, 0, 0)),
                _ => {}
            }
        }
    } else {
        rest = pending;
    }
    for &(c, b, s) in &writes {
        // The cell was checked against the palette; a write cannot fail.
        let _ = v.set_with_state(c, b, s, tick);
        for d in AROUND {
            rest.insert(add(c, d));
        }
        rest.insert(c);
    }
    v.fluid.pending = rest.into_iter().collect();
    v.fluid.seen = tick;
    true
}

/// Every cell of an edited chunk that a liquid may now change: a liquid that is not a source, or
/// an open cell beside a liquid (liquids.md 4.1).
fn take_edits(v: &Voxels, flowing: &[(u8, Flow)], pending: &mut BTreeSet<Cell>) {
    let liquid = |b: u8| flowing.iter().any(|(f, _)| *f == b);
    for chunk in v.chunks.iter().filter(|c| c.edited > v.fluid.seen) {
        let base = chunk.key.map(|k| k * CHUNK);
        for y in 0..CHUNK {
            for z in 0..CHUNK {
                for x in 0..CHUNK {
                    let c = add(base, [x, y, z]);
                    let (b, s) = v.cell(c);
                    if liquid(b) {
                        if !is_source(s) {
                            pending.insert(c);
                        }
                        // A source's open neighbours may fill, across the chunk's border too.
                        for d in AROUND {
                            let n = add(c, d);
                            let nb = v.get(n);
                            if nb == 0 || v.shape(nb) == BlockShape::Cross {
                                pending.insert(n);
                            }
                        }
                    } else if (b == 0 || v.shape(b) == BlockShape::Cross)
                        && AROUND.iter().any(|d| liquid(v.get(add(c, *d))))
                    {
                        pending.insert(c);
                    }
                }
            }
        }
    }
}

/// `physics.fluids`: each volume's liquids, in id order.
pub(crate) fn fluids(clock: Res<SimClock>, mut volumes: Query<(&EntityId, &mut Voxels)>) {
    let tick = clock.tick.0;
    for (_, mut v) in by_id_mut(&mut volumes) {
        // Read first, so a volume with nothing to do is not marked changed.
        if due(&v, tick) {
            advance(&mut v, tick);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amoris_sim::voxels::Block;

    const STONE: u8 = 1;
    const WATER: u8 = 2;
    const GRASS: u8 = 3;

    fn pool() -> Voxels {
        let mut v = Voxels::default();
        v.palette.push(Block {
            name: "stone".into(),
            solid: true,
            faces: ["s".into(), "s".into(), "s".into()],
            ..Block::default()
        });
        v.palette.push(Block {
            name: "water".into(),
            shape: BlockShape::Liquid,
            faces: ["w".into(), String::new(), String::new()],
            flow: Some(Flow {
                period_ticks: 1,
                ..Flow::default()
            }),
            ..Block::default()
        });
        v.palette.push(Block {
            name: "grass".into(),
            shape: BlockShape::Cross,
            faces: ["g".into(), String::new(), String::new()],
            ..Block::default()
        });
        v.fill([-20, 0, -20], [20, 0, 20], STONE, 0).unwrap();
        v
    }

    fn run(v: &mut Voxels, from: u64, ticks: u64) {
        for t in from..from + ticks {
            advance(v, t);
        }
    }

    fn level(v: &Voxels, c: Cell) -> Option<u8> {
        let (b, s) = v.cell(c);
        (b == WATER).then_some(s)
    }

    #[test]
    fn a_source_spreads_seven_cells_on_flat_ground_and_stops() {
        let mut v = pool();
        v.set([0, 1, 0], WATER, 1).unwrap();
        run(&mut v, 2, 30);
        for x in 0..=7 {
            assert_eq!(level(&v, [x, 1, 0]), u8::try_from(x).ok(), "x = {x}");
        }
        assert_eq!(level(&v, [8, 1, 0]), None);
        assert_eq!(level(&v, [3, 1, 4]), Some(7), "diamond: |x| + |z| = 7");
        assert!(
            v.fluid.pending.is_empty(),
            "settled: {:?}",
            v.fluid.pending.len()
        );
        // Settled water does not touch the volume again.
        let before = v.clone();
        run(&mut v, 40, 5);
        assert_eq!(v, before);
    }

    #[test]
    fn liquid_falls_then_spreads_and_runs_to_the_nearest_hole() {
        let mut v = pool();
        // A source on a 3-cell-high pillar falls and spreads from the bottom as from a source.
        v.fill([0, 1, 0], [0, 3, 0], STONE, 0).unwrap();
        v.set([0, 4, 0], WATER, 1).unwrap();
        run(&mut v, 2, 40);
        assert_eq!(level(&v, [1, 4, 0]), Some(1));
        assert_eq!(level(&v, [2, 4, 0]), None, "it falls at the pillar's edge");
        assert_eq!(level(&v, [1, 3, 0]).map(|s| s & FALLING), Some(FALLING));
        assert_eq!(level(&v, [1, 1, 0]).map(|s| s & FALLING), Some(FALLING));
        assert_eq!(
            level(&v, [5, 1, 0]),
            Some(4),
            "landed falling water spreads like a source"
        );

        // On ground with a hole two cells to +x, the flow goes only that way.
        let mut w = pool();
        w.set([2, 0, 0], 0, 0).unwrap();
        w.set([0, 1, 0], WATER, 1).unwrap();
        run(&mut w, 2, 20);
        assert_eq!(level(&w, [1, 1, 0]), Some(1));
        assert_eq!(level(&w, [-1, 1, 0]), None, "not away from the hole");
        assert_eq!(level(&w, [0, 1, 1]), None);
        assert!(level(&w, [2, 0, 0]).is_some(), "it fills the hole");
        assert_eq!(level(&w, [2, -1, 0]), None, "and stops at the volume's box");
    }

    #[test]
    fn two_sources_renew_a_pool_and_removing_a_source_drains_its_flow() {
        let mut v = pool();
        // A 2 x 2 pool walled in: two sources on a diagonal fill it with sources.
        for (x, z) in [
            (-1, -1),
            (-1, 0),
            (-1, 1),
            (-1, 2),
            (2, -1),
            (2, 0),
            (2, 1),
            (2, 2),
        ] {
            v.set([x, 1, z], STONE, 0).unwrap();
        }
        for x in 0..2 {
            v.set([x, 1, -1], STONE, 0).unwrap();
            v.set([x, 1, 2], STONE, 0).unwrap();
        }
        v.set([0, 1, 0], WATER, 1).unwrap();
        v.set([1, 1, 1], WATER, 1).unwrap();
        run(&mut v, 2, 10);
        for c in [[0, 1, 1], [1, 1, 0]] {
            assert_eq!(level(&v, c), Some(0), "{c:?} renewed");
        }

        let mut w = pool();
        w.set([0, 1, 0], WATER, 1).unwrap();
        run(&mut w, 2, 30);
        w.set([0, 1, 0], 0, 40).unwrap();
        run(&mut w, 41, 40);
        assert!(
            (-8..=8).all(|x| level(&w, [x, 1, 0]).is_none()),
            "the flow drained"
        );
    }

    #[test]
    fn plants_are_washed_away_and_a_restore_continues_the_same_flow() {
        let mut v = pool();
        v.set([2, 1, 0], GRASS, 0).unwrap();
        v.set([0, 1, 0], WATER, 1).unwrap();
        run(&mut v, 2, 3);
        // A copy taken mid-flow (a snapshot) continues exactly as the original.
        let mut copy = v.clone();
        run(&mut v, 5, 30);
        run(&mut copy, 5, 30);
        assert_eq!(v, copy);
        assert_eq!(level(&v, [2, 1, 0]), Some(2), "the plant gave way");
    }

    #[test]
    fn a_large_step_continues_at_the_next_period() {
        let mut v = pool();
        v.fluid.pending = (0..i32::try_from(MAX_STEP_CELLS).unwrap() + 10)
            .map(|i| [i, 50, 0])
            .collect();
        advance(&mut v, 1);
        assert_eq!(v.fluid.pending.len(), 10, "the cells past the cap wait");
    }
}
