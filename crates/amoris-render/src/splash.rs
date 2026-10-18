//! Splashes over block liquids (docs/spec/splashes.md): where a liquid with a `splash` look enters a
//! cell, particles gush; under a falling stream's foot they churn. The cellular automaton of
//! `physics.fluids` stays the rules; these particles, simulated by `fluid.rs` in up to four regions
//! near the camera, are only what the player sees, and vanish into the liquid the rules placed.
//!
//! What emits is derived from the chunks the renderer receives, by pure functions of the old and
//! new cells ([`gushes`], [`feet`]), so it is tested without a GPU.

use std::collections::BTreeMap;

use amoris_assets::frame::BlockLook;
use amoris_assets::voxel_mesh::CHUNK;

use crate::fluid::{FluidTuning, Region, SLOTS, SOLID, SolverSpec};

const LIQUID: u8 = 2;
const CROSS: u8 = 1;
const FALLING: u8 = 0b1000;
const LEVEL: u8 = 0b0111;

/// Particles per gush.
pub const GUSH: usize = 24;
/// Particles a stream foot emits per second.
pub const FOOT_RATE: f32 = 480.0;
/// Nothing emits farther from the camera, metres.
pub const REACH_M: f32 = 48.0;
/// At most this many particles are emitted per frame.
pub const PER_FRAME: usize = 2048;
/// At most this many stream feet emit.
pub const FEET: usize = 64;
/// Blocks on a region's edge, and solver cells on a block's.
pub const REGION_BLOCKS: i32 = 16;
pub const PER_BLOCK: u32 = 4;
/// A particle's life, seconds.
pub const LIFETIME_S: f32 = 4.0;

/// Whether palette entry `b` is a liquid that splashes.
fn splashes(looks: &[BlockLook], b: u8) -> Option<[f32; 3]> {
    looks
        .get(usize::from(b))
        .filter(|l| l.shape == LIQUID)
        .and_then(|l| l.splash)
}

/// A cell a liquid entered: its cell, its palette entry, the horizontal direction it came from
/// (zero: none, a source set by hand) and whether it falls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gush {
    pub cell: [i32; 3],
    pub liquid: u8,
    pub dir: [i32; 2],
    pub falling: bool,
}

/// The gushes of a chunk replaced (key `key`): cells that held air or a plant (`old`) and hold a
/// splashing liquid now (`block`, `state`, which also answer for the neighbours). The feeder is the
/// horizontal neighbour of the same liquid with the lowest level (falling liquid counts as 0); a
/// tie takes the first of x-, x+, z-, z+.
pub fn gushes(
    key: [i32; 3],
    looks: &[BlockLook],
    old: impl Fn([i32; 3]) -> u8,
    block: impl Fn([i32; 3]) -> u8,
    state: impl Fn([i32; 3]) -> u8,
) -> Vec<Gush> {
    let mut out = Vec::new();
    for y in 0..CHUNK {
        for z in 0..CHUNK {
            for x in 0..CHUNK {
                let c = [key[0] * CHUNK + x, key[1] * CHUNK + y, key[2] * CHUNK + z];
                let b = block(c);
                if splashes(looks, b).is_none() {
                    continue;
                }
                let was = old(c);
                let open = was == 0
                    || looks
                        .get(usize::from(was))
                        .is_some_and(|l| l.shape == CROSS);
                if !open {
                    continue;
                }
                let mut dir = [0, 0];
                let mut best = u8::MAX;
                for d in [[-1, 0], [1, 0], [0, -1], [0, 1]] {
                    let n = [c[0] + d[0], c[1], c[2] + d[1]];
                    if block(n) != b {
                        continue;
                    }
                    let s = state(n);
                    let level = if s & FALLING != 0 { 0 } else { s & LEVEL };
                    if level < best {
                        best = level;
                        // Away from the feeder.
                        dir = [-d[0], -d[1]];
                    }
                }
                out.push(Gush {
                    cell: c,
                    liquid: b,
                    dir,
                    falling: state(c) & FALLING != 0,
                });
            }
        }
    }
    out
}

/// A falling stream's foot: its lowest falling cell, its liquid and its height in cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Foot {
    pub liquid: u8,
    pub height: u32,
}

/// The stream feet in chunk `key`: falling cells of a splashing liquid whose cell below is not
/// falling liquid of the same kind; the height counts the same liquid upward (at most 64).
pub fn feet(
    key: [i32; 3],
    looks: &[BlockLook],
    block: impl Fn([i32; 3]) -> u8,
    state: impl Fn([i32; 3]) -> u8,
) -> Vec<([i32; 3], Foot)> {
    let mut out = Vec::new();
    for y in 0..CHUNK {
        for z in 0..CHUNK {
            for x in 0..CHUNK {
                let c = [key[0] * CHUNK + x, key[1] * CHUNK + y, key[2] * CHUNK + z];
                let b = block(c);
                if state(c) & FALLING == 0 || splashes(looks, b).is_none() {
                    continue;
                }
                let below = [c[0], c[1] - 1, c[2]];
                if block(below) == b && state(below) & FALLING != 0 {
                    continue;
                }
                let mut height = 1;
                while height < 64 && block([c[0], c[1] + height as i32, c[2]]) == b {
                    height += 1;
                }
                out.push((c, Foot { liquid: b, height }));
            }
        }
    }
    out
}

/// The surface of the liquid in a cell, in ninths of the cell (liquids.md 6): a falling cell or
/// one under the same liquid is full.
fn ninths(b: u8, s: u8, above: u8) -> u8 {
    if s & FALLING != 0 || above == b {
        9
    } else {
        8 - (s & LEVEL)
    }
}

/// A region's mask: a byte per block of the `REGION_BLOCKS` cube from `lo`, x fastest, then y, then
/// z: a full cube solid, a liquid its surface in ninths, the rest open.
pub fn mask(
    lo: [i32; 3],
    looks: &[BlockLook],
    block: impl Fn([i32; 3]) -> u8,
    state: impl Fn([i32; 3]) -> u8,
) -> Vec<u8> {
    let n = REGION_BLOCKS;
    let mut out = Vec::with_capacity((n * n * n) as usize);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let c = [lo[0] + x, lo[1] + y, lo[2] + z];
                let b = block(c);
                out.push(match looks.get(usize::from(b)).map(|l| l.shape) {
                    Some(0) if b != 0 => SOLID,
                    Some(LIQUID) => ninths(b, state(c), block([c[0], c[1] + 1, c[2]])),
                    _ => 0,
                });
            }
        }
    }
    out
}

/// A small generator of the renderer's own (rng.md 8): xorshift, seeded by the frame.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    /// Uniform in [0, 1).
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// What a volume looks like to the splashes: its place, cell, looks, cells and states.
pub trait Cells {
    fn origin(&self) -> [f32; 3];
    fn cell_m(&self) -> f32;
    fn looks(&self) -> &[BlockLook];
    fn block(&self, c: [i32; 3]) -> u8;
    fn state(&self, c: [i32; 3]) -> u8;
    /// The keys of the chunks it holds.
    fn keys(&self) -> Vec<[i32; 3]>;
}

/// What changed in the renderer's volumes since the last frame, for the splashes.
#[derive(Default)]
pub struct Changes {
    /// Gushes, by volume.
    pub gushes: Vec<(u64, Gush)>,
    /// Chunks replaced, by volume.
    pub chunks: Vec<(u64, [i32; 3])>,
    /// Volumes removed, moved, given new looks or seen for the first time: their regions close
    /// and their feet are found again.
    pub fresh: Vec<u64>,
}

impl Changes {
    pub fn extend(&mut self, other: Changes) {
        self.gushes.extend(other.gushes);
        self.chunks.extend(other.chunks);
        self.fresh.extend(other.fresh);
    }
}

/// Particles to spawn in a region: world positions and velocities (m/s).
pub type Spawns = Vec<([f32; 3], [f32; 3])>;

/// The splashes' state on the CPU: the regions' places, the stream feet, the emissions' carry.
pub struct Splashes {
    /// The volume and lowest block of each open region.
    places: [Option<(u64, [i32; 3])>; SLOTS],
    /// Each volume's stream feet, with the fraction of a particle each carries to the next frame.
    feet: BTreeMap<u64, BTreeMap<[i32; 3], (Foot, f32)>>,
    pending: Changes,
    frame: u64,
    /// The frame each slot last spawned in: the one longest without gives way when all are open.
    used: [u64; SLOTS],
}

/// What one frame asks of the solver: regions to open (slot, region, mask), masks to replace, and
/// particles to spawn per slot; regions to close come first.
#[derive(Default)]
pub struct Work {
    pub close: Vec<usize>,
    pub open: Vec<(usize, Region, Vec<u8>)>,
    pub masks: Vec<(usize, Vec<u8>)>,
    pub spawn: Vec<(usize, Spawns)>,
}

/// The solver splashes run on.
pub fn solver() -> SolverSpec {
    let n = REGION_BLOCKS as u32 * PER_BLOCK;
    SolverSpec {
        dims: [n; 3],
        per_block: PER_BLOCK,
        capacity: 32_768,
        substeps: 4,
        lifetime_s: LIFETIME_S,
        spacing: 0.5,
        tuning: FluidTuning::default(),
    }
}

impl Default for Splashes {
    fn default() -> Self {
        Splashes {
            places: [None; SLOTS],
            feet: BTreeMap::new(),
            pending: Changes::default(),
            frame: 0,
            used: [0; SLOTS],
        }
    }
}

impl Splashes {
    /// Forgets everything (a reset frame).
    pub fn clear(&mut self) {
        self.places = [None; SLOTS];
        self.feet.clear();
        self.pending = Changes::default();
    }

    /// Changes the renderer's volumes went through; [`Splashes::frame`] uses them.
    pub fn note(&mut self, changes: Changes) {
        self.pending.extend(changes);
    }

    /// Stream feet now standing, by volume (for checks).
    pub fn feet(&self, volume: u64) -> Vec<([i32; 3], Foot)> {
        self.feet
            .get(&volume)
            .map(|f| f.iter().map(|(c, (foot, _))| (*c, *foot)).collect())
            .unwrap_or_default()
    }

    /// One frame: regions to close (`quiet`: whether each slot's particles can no longer live),
    /// feet found again, gushes and feet turned into particles near `eye` (world metres) in regions
    /// opened as needed. `volume` answers for each volume the renderer holds; `enabled` false
    /// emits nothing (the pending changes still update the feet).
    pub fn frame<'a>(
        &mut self,
        volume: impl Fn(u64) -> Option<&'a dyn Cells>,
        eye: [f32; 3],
        dt: f32,
        quiet: [bool; SLOTS],
        enabled: bool,
    ) -> Work {
        self.frame += 1;
        let mut rng = Rng::new(self.frame);
        let mut work = Work::default();
        let changes = std::mem::take(&mut self.pending);
        // Volumes gone, moved or new: their regions close, their feet are found from scratch.
        let mut fresh = changes.fresh.clone();
        fresh.sort_unstable();
        fresh.dedup();
        for id in &fresh {
            for slot in 0..SLOTS {
                if self.places[slot].is_some_and(|(v, _)| v == *id) {
                    self.places[slot] = None;
                    work.close.push(slot);
                }
            }
            self.feet.remove(id);
            if let Some(v) = volume(*id) {
                let feet = self.feet.entry(*id).or_default();
                for key in v.keys() {
                    for (c, f) in feet_of(v, key) {
                        feet.insert(c, (f, 0.0));
                    }
                }
            }
        }
        // Regions whose particles are all gone close.
        for (slot, place) in self.places.iter_mut().enumerate() {
            if quiet[slot] && place.is_some() {
                *place = None;
                work.close.push(slot);
            }
        }
        // Chunks replaced: their feet and those of the chunks above and below are found again, and
        // the masks of the regions they touch are written again.
        let mut chunks: Vec<(u64, [i32; 3])> = changes
            .chunks
            .iter()
            .filter(|(id, _)| !fresh.contains(id))
            .flat_map(|&(id, k)| [-1, 0, 1].map(move |dy| (id, [k[0], k[1] + dy, k[2]])))
            .collect();
        chunks.sort_unstable();
        chunks.dedup();
        for &(id, key) in &chunks {
            let Some(v) = volume(id) else { continue };
            let feet = self.feet.entry(id).or_default();
            let lo = key.map(|k| k * CHUNK);
            let old: Vec<[i32; 3]> = feet
                .range(lo..)
                .map(|(c, _)| *c)
                .take_while(|c| c[0] < lo[0] + CHUNK)
                .filter(|c| (0..3).all(|a| c[a].div_euclid(CHUNK) == key[a]))
                .collect();
            let mut kept: BTreeMap<[i32; 3], f32> = BTreeMap::new();
            for c in old {
                if let Some((_, carry)) = feet.remove(&c) {
                    kept.insert(c, carry);
                }
            }
            for (c, f) in feet_of(v, key) {
                feet.insert(c, (f, kept.get(&c).copied().unwrap_or(0.0)));
            }
        }
        let mut remask: Vec<usize> = Vec::new();
        for slot in 0..SLOTS {
            let Some((id, lo)) = self.places[slot] else {
                continue;
            };
            let touched = changes.chunks.iter().any(|&(v, k)| {
                v == id
                    && (0..3)
                        .all(|a| k[a] * CHUNK < lo[a] + REGION_BLOCKS && (k[a] + 1) * CHUNK > lo[a])
            });
            if touched {
                remask.push(slot);
            }
        }
        for slot in remask {
            if let Some((id, lo)) = self.places[slot]
                && let Some(v) = volume(id)
            {
                work.masks.push((slot, region_mask(v, lo)));
            }
        }
        if !enabled {
            return work;
        }
        // What emits this frame, nearest first: gushes, then feet.
        let mut emits: Vec<(f32, u64, Emit)> = Vec::new();
        for &(id, g) in &changes.gushes {
            if fresh.contains(&id) {
                continue;
            }
            let Some(v) = volume(id) else { continue };
            if let Some(d) = near(v, g.cell, eye) {
                emits.push((d, id, Emit::Gush(g)));
            }
        }
        let mut feet: Vec<(f32, u64, [i32; 3])> = Vec::new();
        for (&id, fs) in &self.feet {
            let Some(v) = volume(id) else { continue };
            for c in fs.keys() {
                if let Some(d) = near(v, *c, eye) {
                    feet.push((d, id, *c));
                }
            }
        }
        feet.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        feet.truncate(FEET);
        for (d, id, c) in feet {
            let Some(entry) = self.feet.get_mut(&id).and_then(|f| f.get_mut(&c)) else {
                continue;
            };
            entry.1 += FOOT_RATE * dt;
            let n = entry.1.floor();
            entry.1 -= n;
            if n >= 1.0 {
                emits.push((d, id, Emit::Foot(c, entry.0, n as usize)));
            }
        }
        emits.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut budget = PER_FRAME;
        let mut spawns: BTreeMap<usize, Spawns> = BTreeMap::new();
        for (_, id, e) in emits {
            if budget == 0 {
                break;
            }
            let Some(v) = volume(id) else { continue };
            let (cell, liquid) = match e {
                Emit::Gush(g) => (g.cell, g.liquid),
                Emit::Foot(c, f, _) => (c, f.liquid),
            };
            let Some(absorb) = splashes(v.looks(), liquid) else {
                continue;
            };
            let Some((slot, opened)) = self.region_for(id, v, cell, absorb, quiet, &mut work)
            else {
                continue;
            };
            if opened {
                // A slot taken over this frame: what it was given before belongs to the old region.
                spawns.remove(&slot);
            }
            self.used[slot] = self.frame;
            let out = spawns.entry(slot).or_default();
            let before = out.len();
            match e {
                Emit::Gush(g) => gush_particles(v, g, &mut rng, out),
                Emit::Foot(c, f, n) => foot_particles(v, c, f, n, &mut rng, out),
            }
            let added = out.len() - before;
            if added > budget {
                out.truncate(before + budget);
            }
            budget = budget.saturating_sub(added);
        }
        work.spawn = spawns.into_iter().filter(|(_, s)| !s.is_empty()).collect();
        work
    }

    /// The slot of a region of volume `id` that holds `cell` within its inner blocks, opening one
    /// centred on it when none does (closing the one longest without a spawn when all are open),
    /// and whether it opened.
    fn region_for(
        &mut self,
        id: u64,
        v: &dyn Cells,
        cell: [i32; 3],
        absorb: [f32; 3],
        quiet: [bool; SLOTS],
        work: &mut Work,
    ) -> Option<(usize, bool)> {
        let inner = |lo: [i32; 3]| {
            (0..3).all(|a| cell[a] >= lo[a] + 2 && cell[a] < lo[a] + REGION_BLOCKS - 2)
        };
        if let Some(slot) =
            (0..SLOTS).find(|&s| self.places[s].is_some_and(|(v2, lo)| v2 == id && inner(lo)))
        {
            return Some((slot, false));
        }
        let slot = (0..SLOTS)
            .find(|&s| self.places[s].is_none())
            .or_else(|| (0..SLOTS).find(|&s| quiet[s]))
            // All busy: the one longest without a spawn gives way (its particles die).
            .or_else(|| (0..SLOTS).min_by_key(|&s| self.used[s]))?;
        let lo = cell.map(|c| c - REGION_BLOCKS / 2);
        self.places[slot] = Some((id, lo));
        let cell_m = v.cell_m() / PER_BLOCK as f32;
        let origin = [0, 1, 2].map(|a| v.origin()[a] + lo[a] as f32 * v.cell_m());
        work.open.retain(|(s, ..)| *s != slot);
        work.open.push((
            slot,
            Region {
                origin,
                cell_m,
                absorb,
            },
            region_mask(v, lo),
        ));
        Some((slot, true))
    }
}

enum Emit {
    Gush(Gush),
    Foot([i32; 3], Foot, usize),
}

fn region_mask(v: &dyn Cells, lo: [i32; 3]) -> Vec<u8> {
    mask(lo, v.looks(), |c| v.block(c), |c| v.state(c))
}

fn feet_of(v: &dyn Cells, key: [i32; 3]) -> Vec<([i32; 3], Foot)> {
    feet(key, v.looks(), |c| v.block(c), |c| v.state(c))
}

/// The distance from `eye` to a cell's centre, if within reach.
fn near(v: &dyn Cells, cell: [i32; 3], eye: [f32; 3]) -> Option<f32> {
    let p = [0, 1, 2].map(|a| v.origin()[a] + (cell[a] as f32 + 0.5) * v.cell_m());
    let d = ((p[0] - eye[0]).powi(2) + (p[1] - eye[1]).powi(2) + (p[2] - eye[2]).powi(2)).sqrt();
    (d <= REACH_M).then_some(d)
}

fn world(v: &dyn Cells, c: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|a| v.origin()[a] + c[a] * v.cell_m())
}

/// A gush: a clump at the new liquid's surface, thrown away from its feeder at 1.5 m/s; a falling
/// cell's clump starts under it (where the stream goes next) and moves down at 3 m/s.
fn gush_particles(v: &dyn Cells, g: Gush, rng: &mut Rng, out: &mut Spawns) {
    let c = g.cell.map(|x| x as f32);
    let (centre, vel) = if g.falling {
        let below = [g.cell[0], g.cell[1] - 1, g.cell[2]];
        if v.block(below) != 0 {
            return;
        }
        ([c[0] + 0.5, c[1] - 0.2, c[2] + 0.5], [0.0, -3.0, 0.0])
    } else {
        let s = v.state(g.cell);
        let top = f32::from(8 - (s & LEVEL)) / 9.0;
        let d = g.dir.map(|x| x as f32);
        (
            [
                c[0] + 0.5 + 0.25 * d[0],
                c[1] + top + 0.12,
                c[2] + 0.5 + 0.25 * d[1],
            ],
            [1.5 * d[0], 0.4, 1.5 * d[1]],
        )
    };
    // A clump of 2 x 1 x 2 solver cells: eight particles a cell, the solver's rest density.
    let half = [0.25, 0.125, 0.25];
    for _ in 0..GUSH {
        let p = [0, 1, 2].map(|a| centre[a] + rng.range(-half[a], half[a]));
        let j = [0, 1, 2].map(|a| vel[a] + rng.range(-0.5, 0.5));
        out.push((world(v, p), j));
    }
}

/// A stream foot's spray: from the sides of the foot that are not solid, at the height of the
/// liquid beside it (or the foot's bottom over air), outward at 0.5 to 2.5 m/s and up at up to
/// 0.4 sqrt(2 g h), at most 4 m/s.
fn foot_particles(v: &dyn Cells, c: [i32; 3], f: Foot, n: usize, rng: &mut Rng, out: &mut Spawns) {
    let looks = v.looks();
    let mut sides: Vec<([i32; 2], f32)> = Vec::new();
    for d in [[-1, 0], [1, 0], [0, -1], [0, 1]] {
        let s = [c[0] + d[0], c[1], c[2] + d[1]];
        let b = v.block(s);
        let shape = looks.get(usize::from(b)).map(|l| l.shape);
        let height = match shape {
            _ if b == 0 => 0.0,
            Some(LIQUID) => f32::from(ninths(b, v.state(s), v.block([s[0], s[1] + 1, s[2]]))) / 9.0,
            Some(CROSS) => 0.0,
            _ => continue,
        };
        if height < 1.0 {
            sides.push((d, height));
        }
    }
    if sides.is_empty() {
        return;
    }
    let h = f.height as f32 * v.cell_m();
    let up = (0.4 * (2.0 * 9.81 * h).sqrt()).min(4.0);
    let base = c.map(|x| x as f32);
    for i in 0..n {
        let (d, height) = sides[i % sides.len()];
        let along = rng.range(0.1, 0.9);
        let p = if d[0] != 0 {
            [
                base[0] + if d[0] > 0 { 1.05 } else { -0.05 },
                base[1] + height + 0.05,
                base[2] + along,
            ]
        } else {
            [
                base[0] + along,
                base[1] + height + 0.05,
                base[2] + if d[1] > 0 { 1.05 } else { -0.05 },
            ]
        };
        let out_speed = rng.range(0.5, 2.5);
        let side = rng.range(-0.4, 0.4);
        let vel = [
            d[0] as f32 * out_speed + d[1] as f32 * side,
            up * rng.range(0.5, 1.0),
            d[1] as f32 * out_speed + d[0] as f32 * side,
        ];
        out.push((world(v, p), vel));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as HashMap;

    const STONE: u8 = 1;
    const WATER: u8 = 2;
    const FLOWER: u8 = 3;
    const LAVA: u8 = 4;

    fn looks() -> Vec<BlockLook> {
        let look = |shape, splash| BlockLook {
            shape,
            faces: Default::default(),
            fog: None,
            splash,
        };
        vec![
            BlockLook::default(),
            look(0, None),
            look(LIQUID, Some([6.0, 1.8, 0.9])),
            look(CROSS, None),
            look(LIQUID, None),
        ]
    }

    #[derive(Default, Clone)]
    struct World {
        looks: Vec<BlockLook>,
        cells: HashMap<[i32; 3], (u8, u8)>,
    }

    impl World {
        fn new() -> World {
            World {
                looks: looks(),
                cells: HashMap::new(),
            }
        }

        fn set(&mut self, c: [i32; 3], b: u8, s: u8) {
            self.cells.insert(c, (b, s));
        }

        /// Stone under y = 0 over x, z in 0..16.
        fn floor(mut self) -> World {
            for x in 0..16 {
                for z in 0..16 {
                    self.set([x, -1, z], STONE, 0);
                }
            }
            self
        }
    }

    impl Cells for World {
        fn origin(&self) -> [f32; 3] {
            [0.0; 3]
        }
        fn cell_m(&self) -> f32 {
            1.0
        }
        fn looks(&self) -> &[BlockLook] {
            &self.looks
        }
        fn block(&self, c: [i32; 3]) -> u8 {
            self.cells.get(&c).map_or(0, |x| x.0)
        }
        fn state(&self, c: [i32; 3]) -> u8 {
            self.cells.get(&c).map_or(0, |x| x.1)
        }
        fn keys(&self) -> Vec<[i32; 3]> {
            let mut k: Vec<[i32; 3]> = self
                .cells
                .keys()
                .map(|c| c.map(|x| x.div_euclid(CHUNK)))
                .collect();
            k.sort_unstable();
            k.dedup();
            k
        }
    }

    fn gushes_of(old: &World, new: &World, key: [i32; 3]) -> Vec<Gush> {
        gushes(
            key,
            &new.looks,
            |c| old.block(c),
            |c| new.block(c),
            |c| new.state(c),
        )
    }

    #[test]
    fn water_entering_air_gushes_away_from_its_feeder() {
        let mut old = World::new().floor();
        old.set([4, 0, 4], WATER, 0);
        let mut new = old.clone();
        new.set([5, 0, 4], WATER, 1);
        let g = gushes_of(&old, &new, [0, 0, 0]);
        assert_eq!(
            g,
            vec![Gush {
                cell: [5, 0, 4],
                liquid: WATER,
                dir: [1, 0],
                falling: false
            }]
        );
    }

    #[test]
    fn a_plant_washed_away_gushes_and_a_falling_cell_gushes_down() {
        let mut old = World::new().floor();
        old.set([2, 0, 2], FLOWER, 0);
        old.set([8, 5, 8], WATER, 0);
        let mut new = old.clone();
        new.set([2, 0, 2], WATER, 0);
        new.set([8, 4, 8], WATER, FALLING);
        let g = gushes_of(&old, &new, [0, 0, 0]);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].cell, [2, 0, 2]);
        assert_eq!(g[0].dir, [0, 0], "a source set by hand has no feeder");
        assert_eq!(g[1].cell, [8, 4, 8]);
        assert!(g[1].falling);
    }

    #[test]
    fn no_gush_for_a_liquid_without_splash_or_a_cell_that_was_liquid() {
        let mut old = World::new().floor();
        old.set([3, 0, 3], WATER, 0);
        let mut new = old.clone();
        new.set([3, 0, 3], WATER, 2);
        new.set([6, 0, 6], LAVA, 0);
        assert!(gushes_of(&old, &new, [0, 0, 0]).is_empty());
    }

    #[test]
    fn a_feeder_is_the_lowest_level_first_in_order() {
        let mut old = World::new().floor();
        old.set([4, 0, 5], WATER, 3);
        old.set([6, 0, 5], WATER, 2);
        old.set([5, 0, 4], WATER, 2);
        let mut new = old.clone();
        new.set([5, 0, 5], WATER, 3);
        let g = gushes_of(&old, &new, [0, 0, 0]);
        // x+ (level 2) comes before z- (level 2) and beats x- (level 3): away from x+ is x-.
        assert_eq!(g[0].dir, [-1, 0]);
    }

    #[test]
    fn feet_stand_under_falling_columns_with_their_height() {
        let mut w = World::new().floor();
        // A stream of 5 cells from y = 6 down to y = 2, over a pool source at y = 1.
        w.set([3, 1, 3], WATER, 0);
        for y in 2..7 {
            w.set([3, y, 3], WATER, FALLING);
        }
        w.set([3, 7, 3], WATER, 0);
        // A stream onto the floor.
        for y in 0..3 {
            w.set([10, y, 10], WATER, FALLING);
        }
        let f = feet([0, 0, 0], &w.looks, |c| w.block(c), |c| w.state(c));
        assert_eq!(
            f,
            vec![
                (
                    [10, 0, 10],
                    Foot {
                        liquid: WATER,
                        height: 3
                    }
                ),
                (
                    [3, 2, 3],
                    Foot {
                        liquid: WATER,
                        height: 6
                    }
                ),
            ]
        );
    }

    #[test]
    fn masks_hold_solids_and_surfaces() {
        let mut w = World::new().floor();
        w.set([1, 0, 0], WATER, 0);
        w.set([2, 0, 0], WATER, 3);
        w.set([3, 0, 0], WATER, FALLING);
        w.set([4, 0, 0], WATER, 1);
        w.set([4, 1, 0], WATER, FALLING);
        w.set([5, 0, 0], FLOWER, 0);
        let m = mask([0, -1, 0], &w.looks, |c| w.block(c), |c| w.state(c));
        let at =
            |x: i32, y: i32, z: i32| m[(x + REGION_BLOCKS * (y + 1 + REGION_BLOCKS * z)) as usize];
        assert_eq!(at(0, -1, 0), SOLID);
        assert_eq!(at(0, 0, 0), 0);
        assert_eq!(at(1, 0, 0), 8);
        assert_eq!(at(2, 0, 0), 5);
        assert_eq!(at(3, 0, 0), 9);
        assert_eq!(at(4, 0, 0), 9, "under the same liquid: full");
        assert_eq!(at(5, 0, 0), 0);
    }

    fn frame(s: &mut Splashes, w: &World, eye: [f32; 3], enabled: bool) -> Work {
        s.frame(
            |id| (id == 1).then_some(w as &dyn Cells),
            eye,
            1.0 / 60.0,
            [false; SLOTS],
            enabled,
        )
    }

    fn count(work: &Work) -> usize {
        work.spawn.iter().map(|(_, s)| s.len()).sum()
    }

    #[test]
    fn a_gush_opens_a_region_and_spawns_near_its_cell() {
        let mut old = World::new().floor();
        old.set([4, 0, 4], WATER, 0);
        let mut new = old.clone();
        new.set([5, 0, 4], WATER, 1);
        let mut s = Splashes::default();
        s.note(Changes {
            gushes: gushes_of(&old, &new, [0, 0, 0])
                .into_iter()
                .map(|g| (1, g))
                .collect(),
            chunks: vec![(1, [0, 0, 0])],
            fresh: Vec::new(),
        });
        let work = frame(&mut s, &new, [5.0, 2.0, 4.0], true);
        assert_eq!(work.open.len(), 1);
        let (slot, region, mask) = &work.open[0];
        assert_eq!(region.origin, [-3.0, -8.0, -4.0]);
        assert_eq!(region.cell_m, 0.25);
        assert_eq!(region.absorb, [6.0, 1.8, 0.9]);
        assert_eq!(
            mask.len(),
            (REGION_BLOCKS * REGION_BLOCKS * REGION_BLOCKS) as usize
        );
        assert_eq!(count(&work), GUSH);
        let (s0, spawned) = &work.spawn[0];
        assert_eq!(s0, slot);
        for (p, v) in spawned {
            assert!(
                (5.0..6.0).contains(&p[0]) && (0.75..1.1).contains(&p[1]),
                "{p:?}"
            );
            assert!(v[0] > 0.9, "thrown away from the feeder: {v:?}");
        }
    }

    #[test]
    fn nothing_emits_far_away_when_disabled_or_from_a_fresh_volume() {
        let mut old = World::new().floor();
        old.set([4, 0, 4], WATER, 0);
        let mut new = old.clone();
        new.set([5, 0, 4], WATER, 1);
        let gush: Vec<(u64, Gush)> = gushes_of(&old, &new, [0, 0, 0])
            .into_iter()
            .map(|g| (1, g))
            .collect();
        for (eye, enabled, fresh) in [
            ([100.0, 0.0, 0.0], true, false),
            ([5.0, 2.0, 4.0], false, false),
            ([5.0, 2.0, 4.0], true, true),
        ] {
            let mut s = Splashes::default();
            s.note(Changes {
                gushes: gush.clone(),
                chunks: vec![(1, [0, 0, 0])],
                fresh: if fresh { vec![1] } else { Vec::new() },
            });
            let work = frame(&mut s, &new, eye, enabled);
            assert_eq!(count(&work), 0, "{eye:?} {enabled} {fresh}");
            assert!(work.open.is_empty());
        }
    }

    #[test]
    fn a_foot_sprays_at_its_rate_and_the_cap_holds() {
        let mut w = World::new().floor();
        w.set([3, 0, 3], WATER, 0);
        for y in 1..9 {
            w.set([3, y, 3], WATER, FALLING);
        }
        let mut s = Splashes::default();
        s.note(Changes {
            fresh: vec![1],
            ..Changes::default()
        });
        let mut total = 0;
        for _ in 0..60 {
            total += count(&frame(&mut s, &w, [3.0, 4.0, 8.0], true));
        }
        assert_eq!(s.feet(1).len(), 1);
        assert!(
            (FOOT_RATE as usize - 2..=FOOT_RATE as usize).contains(&total),
            "{total}"
        );
        // Fifty gushes at once, then two hundred: the cap.
        let mut old = World::new().floor();
        let mut new = old.clone();
        let mut gush = Vec::new();
        for x in 0..10 {
            for z in 0..5 {
                old.set([x, 0, z], 0, 0);
                new.set([x, 0, z], WATER, 0);
            }
        }
        gush.extend(gushes_of(&old, &new, [0, 0, 0]).into_iter().map(|g| (1, g)));
        assert_eq!(gush.len(), 50);
        let mut s = Splashes::default();
        s.note(Changes {
            gushes: gush,
            chunks: vec![(1, [0, 0, 0])],
            fresh: Vec::new(),
        });
        assert_eq!(
            count(&frame(&mut s, &new, [5.0, 2.0, 5.0], true)),
            GUSH * 50
        );
        let mut many = Vec::new();
        for i in 0..200 {
            many.push((
                1,
                Gush {
                    cell: [i % 10, 0, i / 20],
                    liquid: WATER,
                    dir: [0, 0],
                    falling: false,
                },
            ));
        }
        let mut s = Splashes::default();
        s.note(Changes {
            gushes: many,
            chunks: Vec::new(),
            fresh: Vec::new(),
        });
        assert_eq!(
            count(&frame(&mut s, &new, [5.0, 2.0, 5.0], true)),
            PER_FRAME
        );
    }
}
