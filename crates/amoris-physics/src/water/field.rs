//! Block liquids on the field (water.md 6): a flowing liquid's cells become the water of the region
//! whose bed is their volume, shared over the columns of their block, and are cleared. A cell no
//! such region covers is an error at load (`water.uncovered`) and is cleared with an event later.

use amoris_contract::{Problem, detail};
use amoris_sim::voxels::{CHUNK, Cell, FALLING, LEVEL, Voxels, locate};
use serde_json::json;

use super::WaterRegion;
use super::bed::Map;

/// The part of a liquid cell at or below its surface (liquids.md 6): `(8 - level) / 9` of a cell
/// that is not falling, the whole cell when it falls or the same liquid lies above it.
pub fn part(v: &Voxels, cell: Cell, block: u8, state: u8) -> f64 {
    if state & FALLING != 0 || v.get([cell[0], cell[1] + 1, cell[2]]) == block {
        return 1.0;
    }
    f64::from(8 - (state & LEVEL)) / 9.0
}

/// Every field liquid cell of the chunks edited at `since` or later, in cell order (y, then z,
/// then x within each chunk, chunks in key order): the cell and its part.
pub fn cells(v: &Voxels, since: u64) -> Vec<(Cell, f64)> {
    let field: Vec<bool> = (0..v.palette.len())
        .map(|b| u8::try_from(b).is_ok_and(|b| v.field_liquid(b)))
        .collect();
    if !field.iter().any(|f| *f) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for chunk in v.chunks.iter().filter(|c| c.edited >= since) {
        if !chunk.cells.0.iter().any(|&b| field[usize::from(b)]) {
            continue;
        }
        let base = chunk.key.map(|k| k * CHUNK);
        for y in 0..CHUNK {
            for z in 0..CHUNK {
                for x in 0..CHUNK {
                    let c = [base[0] + x, base[1] + y, base[2] + z];
                    let (_, at) = locate(c);
                    let b = chunk.cells.0[at];
                    if field[usize::from(b)] {
                        let s = chunk.states.as_ref().map_or(0, |s| s.0[at]);
                        out.push((c, part(v, c, b, s)));
                    }
                }
            }
        }
    }
    out
}

/// The region column of block `(x, z)`'s first column, if the region covers the block.
pub fn covers(r: &WaterRegion, m: &Map, x: i32, z: i32) -> Option<(usize, usize)> {
    let [nx, nz] = r.columns_along();
    let bi = usize::try_from(x - m.block0[0]).ok()?;
    let bk = usize::try_from(z - m.block0[1]).ok()?;
    let (i, k) = (bi * m.per, bk * m.per);
    (i < nx && k < nz).then_some((i, k))
}

/// Adds a cell's water, `part` of a cell of `cell_m_volume` metres, in equal shares to the `per`^2
/// columns of its block from region column `(i, k)`, and wakes their tile.
pub fn pour(r: &mut WaterRegion, m: &Map, (i, k): (usize, usize), part: f64) {
    // A block's water over its per^2 columns, each cell_m^2: each column rises by part * cell.
    let rise = part * m.cell;
    for n in 0..m.per * m.per {
        let (t, c) = r.locate(i + n % m.per, k + n / m.per);
        r.tiles[t].depth[c] += rise;
        r.tiles[t].still = 0;
    }
}

/// `water.uncovered` for a field liquid cell no region over its volume covers.
pub fn uncovered(volume: amoris_sim::EntityId, cell: Cell) -> Problem {
    Problem::new(
        "water.uncovered",
        format!(
            "volume {volume}'s field liquid at {cell:?} is under no water region whose bed is the \
             volume (water.md 6)"
        ),
        detail([("volume", json!(volume)), ("cell", json!(cell))]),
    )
}
