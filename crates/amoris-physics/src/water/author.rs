//! Water declared rather than drawn column by column (water.md 65): beds from a `Terrain`, and
//! water poured to a level from a point. Both act when a region's tiles come from its bed, so a
//! scene names a few numbers where its build script wrote every column.

use amoris_sim::math;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{TILE, WaterRegion, WaterTile};
use crate::terrain::{Terrain, height_at};

/// Water poured at the column under `at` (world x, z) until it stands at `level` (world y): every
/// column joined to that one, side by side, by columns whose beds lie under the level fills to it.
/// A column holding more keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaterFill {
    pub at: [f64; 2],
    pub level: f64,
}

/// The most fills a region holds.
pub const MAX_FILLS: usize = 64;

/// Pours each of the region's fills, in order, over its tiles (corner: world x, z).
pub fn fill(r: &mut WaterRegion, corner: [f64; 2]) {
    let [nx, nz] = r.size.map(|n| n as usize);
    if !r.well_formed() {
        return;
    }
    for f in r.fill.clone() {
        let Some(start) = r.column_at(f.at[0] - corner[0], f.at[1] - corner[1]) else {
            continue;
        };
        let bed = |r: &WaterRegion, (i, k): (usize, usize)| {
            let (t, c) = r.locate(i, k);
            r.tiles[t].bed[c]
        };
        if bed(r, start) >= f.level {
            continue;
        }
        let mut seen = vec![false; nx * nz];
        let mut queue = std::collections::VecDeque::from([start]);
        seen[start.0 + nx * start.1] = true;
        while let Some((i, k)) = queue.pop_front() {
            let (t, c) = r.locate(i, k);
            let tile = &mut r.tiles[t];
            tile.depth[c] = math::max(tile.depth[c], f.level - tile.bed[c]);
            let near = [
                (i.wrapping_sub(1), k),
                (i + 1, k),
                (i, k.wrapping_sub(1)),
                (i, k + 1),
            ];
            for (a, b) in near {
                if a < nx && b < nz && !seen[a + nx * b] && bed(r, (a, b)) < f.level {
                    seen[a + nx * b] = true;
                    queue.push_back((a, b));
                }
            }
        }
    }
}

/// Dry tiles over the terrain (placed at `at`, world): each column's bed is the ground's height at
/// its centre. Then the region's fills.
pub fn from_terrain(r: &mut WaterRegion, corner: [f64; 2], terrain: &Terrain, at: [f64; 3]) {
    let heights = terrain.heights();
    let corners = terrain.corners();
    let [tx, tz] = r.tiles_along();
    let mut tiles = Vec::with_capacity(tx * tz);
    for kz in 0..tz {
        for kx in 0..tx {
            let key = [kx, kz].map(|k| u32::try_from(k).unwrap_or(u32::MAX));
            let mut t = WaterTile::dry(key, 0.0);
            for n in 0..TILE * TILE {
                let (i, k) = (kx * TILE + n % TILE, kz * TILE + n / TILE);
                let x = corner[0] + (column(i) + 0.5) * r.cell_m - at[0];
                let z = corner[1] + (column(k) + 0.5) * r.cell_m - at[2];
                t.bed[n] = at[1] + height_at(&heights, corners, terrain.cell_m, x, z);
            }
            tiles.push(t);
        }
    }
    r.tiles = tiles;
    fill(r, corner);
}

/// A column's index as metres' multiplier.
fn column(i: usize) -> f64 {
    f64::from(u32::try_from(i).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{TerrainFeature, TerrainLook};

    /// A terrain 32 x 16 m at (10, 1, 20), its ground 3 m up with a basin and a channel leaving it.
    fn ground() -> Terrain {
        Terrain {
            size: [64, 32],
            cell_m: 0.5,
            base_m: 3.0,
            features: vec![
                TerrainFeature::Hill {
                    at: [8.0, 8.0],
                    radius_m: 6.0,
                    height_m: -3.0,
                },
                TerrainFeature::Channel {
                    path: vec![[13.0, 2.5, 8.0], [30.0, 1.5, 8.0]],
                    width_m: 2.0,
                    bank_m: 1.0,
                },
            ],
            look: TerrainLook::default(),
        }
    }

    #[test]
    fn beds_follow_the_terrain_and_a_pour_fills_only_what_it_reaches() {
        // Over the whole terrain: 64 x 32 columns from its corner.
        let mut r = crate::water::tests::region(4, 2, 0.5);
        r.fill = vec![WaterFill {
            at: [18.0, 28.0],
            level: 2.4,
        }];
        let terrain = ground();
        from_terrain(&mut r, [10.0, 20.0], &terrain, [10.0, 1.0, 20.0]);
        assert!(r.well_formed());
        let column = |r: &WaterRegion, x: f64, z: f64| {
            let (i, k) = r.column_at(x - 10.0, z - 20.0).unwrap();
            let (t, c) = r.locate(i, k);
            (r.tiles[t].bed[c], r.tiles[t].depth[c])
        };
        // The basin's floor is 1 m (corner) + 3 - 3 = 1 m; the pour fills it to 2.4 m.
        let (bed, depth) = column(&r, 18.25, 28.25);
        assert!((bed - 1.0).abs() < 0.05, "{bed}");
        assert!((bed + depth - 2.4).abs() < 1e-9, "{depth}");
        // The ground around stays dry, and so does the channel past the basin's rim: its bed
        // starts at 3.5 m, over the level.
        assert_eq!(column(&r, 30.25, 34.25).1, 0.0);
        let (channel_bed, channel_depth) = column(&r, 38.25, 28.25);
        assert!(
            channel_bed < 4.0 && channel_depth == 0.0,
            "{channel_bed} {channel_depth}"
        );
        // Poured higher, the water runs on down the channel as far as its bed lies under the
        // level, and no further.
        r.tiles.clear();
        r.fill[0].level = 3.8;
        from_terrain(&mut r, [10.0, 20.0], &terrain, [10.0, 1.0, 20.0]);
        assert!(
            column(&r, 30.25, 28.25).1 > 0.0,
            "the channel from the basin fills"
        );
        assert_eq!(
            column(&r, 30.25, 34.25).1,
            0.0,
            "the ground beside it does not"
        );
    }
}
