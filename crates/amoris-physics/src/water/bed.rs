//! Beds from block volumes (water.md 5): where a `Voxels` bed's region sits on the volume's grid,
//! the beds read for a region spawned without tiles, and the beds reread over the block columns of
//! the chunks that changed. Solid blocks that air separates from the water never become bed
//! (owner, 2026-10-18).

use amoris_contract::{Problem, detail};
use amoris_sim::math;
use amoris_sim::num::to_i32;
use amoris_sim::voxels::{CHUNK, Voxels};
use serde_json::json;

use super::{BedSource, TILE, WaterRegion, WaterTile};

/// How a region's columns lie over a volume's blocks.
#[derive(Clone, Copy, Debug)]
pub struct Map {
    /// Columns to a block's edge.
    pub per: usize,
    /// The block x and z under the region's column (0, 0).
    pub block0: [i32; 2],
    /// The cells searched, bottom and top.
    pub y_lo: i32,
    pub y_hi: i32,
    /// The volume's origin y and cell edge, metres.
    pub y0: f64,
    pub cell: f64,
}

fn misaligned(message: String, region: [f64; 2], cell_m: f64) -> Problem {
    Problem::new(
        "water.misaligned",
        message,
        detail([("corner", json!(region)), ("cell_m", json!(cell_m))]),
    )
}

/// The map of a `Voxels` bed's region with its corner at `corner` (world x, z) over the volume `v`
/// placed at `origin`: `water.misaligned` unless the corner is on a cell's corner and `cell_m` is
/// the volume's `cell_m / per` (water.md 5). `None` for a bed that is not a volume's.
pub fn map(
    r: &WaterRegion,
    corner: [f64; 2],
    v: &Voxels,
    origin: [f64; 3],
) -> Option<Result<Map, Problem>> {
    let BedSource::Voxels {
        per, y_lo, y_hi, ..
    } = r.bed
    else {
        return None;
    };
    let per_f = f64::from(per);
    if r.cell_m * per_f != v.cell_m {
        return Some(Err(misaligned(
            format!(
                "the region's columns of {} m at {per} a block do not fit the volume's cells of {} \
                 m",
                r.cell_m, v.cell_m
            ),
            corner,
            r.cell_m,
        )));
    }
    let mut block0 = [0; 2];
    for (a, axis) in [(0, 0), (1, 2)] {
        let off = (corner[a] - origin[axis]) / v.cell_m;
        match to_i32(off) {
            Ok(b) => block0[a] = b,
            Err(_) => {
                return Some(Err(misaligned(
                    format!(
                        "the region's corner {corner:?} is not on a corner of the volume's cells \
                         (offset {off} cells along {})",
                        ["x", "z"][a]
                    ),
                    corner,
                    r.cell_m,
                )));
            }
        }
    }
    Some(Ok(Map {
        per: usize::try_from(per).unwrap_or(1),
        block0,
        y_lo,
        y_hi,
        y0: origin[1],
        cell: v.cell_m,
    }))
}

impl Map {
    fn top(&self, y: i32) -> f64 {
        self.y0 + f64::from(y + 1) * self.cell
    }

    fn bottom(&self) -> f64 {
        self.y0 + f64::from(self.y_lo) * self.cell
    }

    /// The block column under region block column `(bi, bk)`.
    fn block(&self, bi: usize, bk: usize) -> [i32; 2] {
        let o = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
        [self.block0[0] + o(bi), self.block0[1] + o(bk)]
    }

    /// The bed at load of block column `(x, z)` (water.md 5): the top of the highest solid block
    /// within the range, or the range's bottom.
    pub fn load_bed(&self, v: &Voxels, x: i32, z: i32) -> f64 {
        for y in (self.y_lo..=self.y_hi).rev() {
            if v.solid(v.get([x, y, z])) {
                return self.top(y);
            }
        }
        self.bottom()
    }

    /// The bed after a change of block column `(x, z)` whose highest surface is `r` (water.md 5):
    /// the solid run going up from the block holding `r` (on a boundary, the block starting at
    /// it), or the first solid block below `r`, or the range's bottom.
    pub fn changed_bed(&self, v: &Voxels, x: i32, z: i32, r: f64) -> f64 {
        let Ok(held) = to_i32(math::clamp(
            ((r - self.y0) / self.cell).floor(),
            f64::from(self.y_lo) - 1.0,
            f64::from(self.y_hi) + 1.0,
        )) else {
            return self.bottom();
        };
        if held < self.y_lo {
            return self.bottom();
        }
        let solid = |y: i32| v.solid(v.get([x, y, z]));
        if held <= self.y_hi && solid(held) {
            let mut y = held;
            while y < self.y_hi && solid(y + 1) {
                y += 1;
            }
            return self.top(y);
        }
        for y in (self.y_lo..=held.min(self.y_hi)).rev() {
            if solid(y) {
                return self.top(y);
            }
        }
        self.bottom()
    }
}

/// Dry tiles over the volume's beds at load (water.md 5), for a region spawned without tiles.
pub fn load(r: &mut WaterRegion, m: &Map, v: &Voxels) {
    let [tx, tz] = r.tiles_along();
    let mut tiles = Vec::with_capacity(tx * tz);
    for kz in 0..tz {
        for kx in 0..tx {
            let key = [kx, kz].map(|k| u32::try_from(k).unwrap_or(u32::MAX));
            let mut t = WaterTile::dry(key, 0.0);
            for lz in 0..TILE {
                for lx in 0..TILE {
                    let (i, k) = (kx * TILE + lx, kz * TILE + lz);
                    let [x, z] = m.block(i / m.per, k / m.per);
                    t.bed[lx + TILE * lz] = m.load_bed(v, x, z);
                }
            }
            tiles.push(t);
        }
    }
    r.tiles = tiles;
}

/// A column's water a rising bed took (water.md 5): its region column and m^3.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Displaced {
    pub column: [usize; 2],
    pub m3: f64,
}

/// The beds the change rule gives the block columns of the chunks `changed` (chunk keys, any y)
/// under the region, where they differ from the region's: each region column and its new bed, in
/// column order. Reads only, so a region whose beds stand is not marked changed.
pub fn rebeds(
    r: &WaterRegion,
    m: &Map,
    v: &Voxels,
    changed: &[[i32; 2]],
) -> Vec<([usize; 2], f64)> {
    let [nx, nz] = r.columns_along();
    let (bx, bz) = (nx / m.per, nz / m.per);
    let mut blocks = std::collections::BTreeSet::new();
    for key in changed {
        for dz in 0..CHUNK {
            for dx in 0..CHUNK {
                let (x, z) = (key[0] * CHUNK + dx, key[1] * CHUNK + dz);
                if let (Ok(bi), Ok(bk)) = (
                    usize::try_from(x - m.block0[0]),
                    usize::try_from(z - m.block0[1]),
                ) && bi < bx
                    && bk < bz
                {
                    blocks.insert((bk, bi, x, z));
                }
            }
        }
    }
    let mut out = Vec::new();
    for (bk, bi, x, z) in blocks {
        let cols = (0..m.per * m.per).map(|n| (bi * m.per + n % m.per, bk * m.per + n / m.per));
        let mut high = f64::MIN;
        for (i, k) in cols.clone() {
            let (t, c) = r.locate(i, k);
            high = math::max(high, r.tiles[t].bed[c] + r.tiles[t].depth[c]);
        }
        let bed = m.changed_bed(v, x, z, high);
        for (i, k) in cols {
            let (t, c) = r.locate(i, k);
            if r.tiles[t].bed[c] != bed {
                out.push(([i, k], bed));
            }
        }
    }
    out.sort_by_key(|(c, _)| (c[1], c[0]));
    out
}

/// Sets the beds `rebeds` gave (water.md 5): a column whose bed rose under water loses the water
/// the solid took, one whose bed fell keeps its depth. Wakes every tile whose bed changed. Returns
/// the water displaced, by column in column order.
pub fn apply(r: &mut WaterRegion, beds: &[([usize; 2], f64)]) -> Vec<Displaced> {
    let area = r.cell_m * r.cell_m;
    let mut out = Vec::new();
    for &([i, k], bed) in beds {
        let (t, c) = r.locate(i, k);
        let tile = &mut r.tiles[t];
        let old = tile.bed[c];
        if bed > old {
            let d = tile.depth[c];
            let kept = math::max(old + d - bed, 0.0);
            if kept < d {
                out.push(Displaced {
                    column: [i, k],
                    m3: (d - kept) * area,
                });
            }
            tile.depth[c] = kept;
        }
        tile.bed[c] = bed;
        tile.still = 0;
    }
    out
}

/// The chunk columns (x and z keys) of the volume's chunks edited at `since` or later.
pub fn edited_since(v: &Voxels, since: u64) -> Vec<[i32; 2]> {
    let mut keys: Vec<[i32; 2]> = v
        .chunks
        .iter()
        .filter(|c| c.edited >= since)
        .map(|c| [c.key[0], c.key[2]])
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}
