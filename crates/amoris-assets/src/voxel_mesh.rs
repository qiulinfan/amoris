//! Chunk meshes of block volumes (docs/spec/voxels.md 7.2): one chunk's cells, with its neighbours'
//! facing layers, become triangles grouped by material. A cube's face is drawn where it meets air, a
//! plant or a liquid; a plant is two crossed quads; a liquid draws the faces it shares with air,
//! plants or another liquid. A liquid's surface stands at its level (liquids.md 6): each top corner
//! at the highest surface of the up to four cells of that liquid around it, so surfaces slope down a
//! flow without gaps. Positions are in cells from the chunk's corner; texture coordinates repeat
//! once per cell face.

// CPU mesh data for the renderer: its floats never reach the world or its hash.
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;

use crate::frame::BlockLook;
use crate::mesh::{MeshData, Vertex};

/// Cells along a chunk's edge.
pub const CHUNK: i32 = 16;
pub const CUBE: u8 = 0;
pub const CROSS: u8 = 1;
pub const LIQUID: u8 = 2;

/// The six faces: normal, then the corners counter-clockwise seen from outside (side faces start
/// at the top), and the texture coordinates of those corners.
const FACES: [([i32; 3], [[f32; 3]; 4]); 6] = [
    (
        [0, 1, 0],
        [[0., 1., 0.], [0., 1., 1.], [1., 1., 1.], [1., 1., 0.]],
    ),
    (
        [0, -1, 0],
        [[0., 0., 0.], [1., 0., 0.], [1., 0., 1.], [0., 0., 1.]],
    ),
    (
        [1, 0, 0],
        [[1., 1., 1.], [1., 0., 1.], [1., 0., 0.], [1., 1., 0.]],
    ),
    (
        [-1, 0, 0],
        [[0., 1., 0.], [0., 0., 0.], [0., 0., 1.], [0., 1., 1.]],
    ),
    (
        [0, 0, 1],
        [[0., 1., 1.], [0., 0., 1.], [1., 0., 1.], [1., 1., 1.]],
    ),
    (
        [0, 0, -1],
        [[1., 1., 0.], [1., 0., 0.], [0., 0., 0.], [0., 1., 0.]],
    ),
];
const UV: [[f32; 2]; 4] = [[0., 0.], [0., 1.], [1., 1.], [1., 0.]];
/// A liquid state's level bits and falling bit (amoris-sim's `voxels::LEVEL`, `FALLING`).
const LEVEL: u8 = 0b0111;
const FALLING: u8 = 0b1000;

/// A liquid cell's surface as a share of the cell: a source at 8/9, level n at (8 - n) / 9, the
/// whole cell when falling or under the same liquid.
fn surface(state: u8, under_same: bool) -> f32 {
    if under_same || state & FALLING != 0 {
        1.0
    } else {
        f32::from(8 - (state & LEVEL)) / 9.0
    }
}

#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

impl Builder {
    fn quad(&mut self, corners: [[f32; 3]; 4], normal: [f32; 3], tangent: [f32; 4]) {
        let base = u32::try_from(self.vertices.len()).unwrap_or(u32::MAX);
        for (p, uv) in corners.into_iter().zip(UV) {
            self.vertices.push(Vertex {
                position: p,
                normal,
                uv,
                tangent,
            });
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// The meshes of chunk `key`, by material, in material order. `get` and `state` answer any cell of
/// the volume (this chunk's and its neighbours'): its block and its state byte; `blocks` are the
/// palette's looks (index 0, air, is unused).
pub fn mesh_chunk(
    key: [i32; 3],
    blocks: &[BlockLook],
    get: impl Fn([i32; 3]) -> u8,
    state: impl Fn([i32; 3]) -> u8,
) -> Vec<(String, MeshData)> {
    let shape = |b: u8| blocks.get(usize::from(b)).map(|l| l.shape);
    let mut groups: BTreeMap<String, Builder> = BTreeMap::new();
    let base = key.map(|k| k * CHUNK);
    for y in 0..CHUNK {
        for z in 0..CHUNK {
            for x in 0..CHUNK {
                let local = [x, y, z];
                let cell = add(base, local);
                let b = get(cell);
                if b == 0 {
                    continue;
                }
                let Some(look) = blocks.get(usize::from(b)) else {
                    continue;
                };
                let o = local.map(|c| c as f32);
                match look.shape {
                    CROSS => {
                        let g = groups.entry(look.faces[0].clone()).or_default();
                        for ((ax, az), (bx, bz)) in
                            [((0.15, 0.15), (0.85, 0.85)), ((0.15, 0.85), (0.85, 0.15))]
                        {
                            let corners = [
                                [o[0] + ax, o[1] + 1.0, o[2] + az],
                                [o[0] + ax, o[1], o[2] + az],
                                [o[0] + bx, o[1], o[2] + bz],
                                [o[0] + bx, o[1] + 1.0, o[2] + bz],
                            ];
                            let len = ((bx - ax) * (bx - ax) + (bz - az) * (bz - az)).sqrt();
                            let t = [(bx - ax) / len, 0.0, (bz - az) / len, 1.0];
                            // Lit as from above, so both sides of the leaves look alike.
                            g.quad(corners, [0.0, 1.0, 0.0], t);
                        }
                    }
                    LIQUID => {
                        // Each top corner's height: the highest surface of this liquid around it.
                        let mut corner = [[0.0f32; 2]; 2];
                        for (cx, row) in corner.iter_mut().enumerate() {
                            for (cz, h) in row.iter_mut().enumerate() {
                                for (i, j) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                                    let n = add(cell, [cx as i32 - 1 + i, 0, cz as i32 - 1 + j]);
                                    if get(n) != b {
                                        continue;
                                    }
                                    let under = get(add(n, [0, 1, 0])) == b;
                                    *h = h.max(surface(state(n), under));
                                }
                            }
                        }
                        for (n, corners) in FACES {
                            let nb = get(add(cell, n));
                            let shows = nb == 0 || (nb != b && shape(nb) != Some(CUBE));
                            if !shows {
                                continue;
                            }
                            let corners = corners.map(|c| {
                                let top = corner[c[0] as usize][c[2] as usize];
                                [
                                    o[0] + c[0],
                                    o[1] + if c[1] > 0.5 { top } else { 0.0 },
                                    o[2] + c[2],
                                ]
                            });
                            let g = groups.entry(look.faces[0].clone()).or_default();
                            g.quad(corners, n.map(|c| c as f32), tangent(n));
                        }
                    }
                    _ => {
                        for (i, (n, corners)) in FACES.iter().enumerate() {
                            let nb = get(add(cell, *n));
                            if nb != 0 && shape(nb) == Some(CUBE) {
                                continue;
                            }
                            let face = match i {
                                0 => &look.faces[0],
                                1 => &look.faces[2],
                                _ => &look.faces[1],
                            };
                            let corners = corners.map(|c| [o[0] + c[0], o[1] + c[1], o[2] + c[2]]);
                            let g = groups.entry(face.clone()).or_default();
                            g.quad(corners, n.map(|c| c as f32), tangent(*n));
                        }
                    }
                }
            }
        }
    }
    groups
        .into_iter()
        .filter(|(_, g)| !g.indices.is_empty())
        .map(|(material, g)| {
            let mut m = MeshData::new(&material, g.vertices, g.indices);
            m.material = None;
            (material, m)
        })
        .collect()
}

/// The direction the texture's u grows along on a face with normal `n`, with the bitangent's sign.
fn tangent(n: [i32; 3]) -> [f32; 4] {
    match n {
        [0, 1, 0] | [0, -1, 0] => [0.0, 0.0, 1.0, 1.0],
        [1, 0, 0] => [0.0, 0.0, -1.0, 1.0],
        [-1, 0, 0] => [0.0, 0.0, 1.0, 1.0],
        [0, 0, 1] => [1.0, 0.0, 0.0, 1.0],
        _ => [-1.0, 0.0, 0.0, 1.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as HashMap;

    fn looks() -> Vec<BlockLook> {
        let cube = |t: &str, s: &str, b: &str| BlockLook {
            shape: CUBE,
            faces: [t.into(), s.into(), b.into()],
            fog: None,
        };
        vec![
            BlockLook::default(),
            cube("grass_top", "grass_side", "dirt"),
            cube("stone", "stone", "stone"),
            BlockLook {
                shape: CROSS,
                faces: ["flower".into(), String::new(), String::new()],
                fog: None,
            },
            BlockLook {
                shape: LIQUID,
                faces: ["water".into(), String::new(), String::new()],
                fog: None,
            },
        ]
    }

    fn quads(meshes: &[(String, MeshData)], material: &str) -> usize {
        meshes
            .iter()
            .find(|(m, _)| m == material)
            .map_or(0, |(_, d)| d.indices.len() / 6)
    }

    #[test]
    fn a_lone_cube_shows_six_faces_by_material() {
        let cells: HashMap<[i32; 3], u8> = [([3, 4, 5], 1)].into();
        let m = mesh_chunk([0, 0, 0], &looks(), |c| *cells.get(&c).unwrap_or(&0), |_| 0);
        assert_eq!(quads(&m, "grass_top"), 1);
        assert_eq!(quads(&m, "grass_side"), 4);
        assert_eq!(quads(&m, "dirt"), 1);
        let top = &m.iter().find(|(k, _)| k == "grass_top").unwrap().1;
        assert!(
            top.vertices
                .iter()
                .all(|v| v.position[1] == 5.0 && v.normal == [0.0, 1.0, 0.0])
        );
    }

    #[test]
    fn faces_between_cubes_are_culled_across_chunk_borders() {
        // A 2 x 1 x 1 bar straddling chunks 0 and 1 along x, meshed from chunk 0.
        let cells: HashMap<[i32; 3], u8> = [([15, 0, 0], 2), ([16, 0, 0], 2)].into();
        let m = mesh_chunk([0, 0, 0], &looks(), |c| *cells.get(&c).unwrap_or(&0), |_| 0);
        assert_eq!(quads(&m, "stone"), 5, "the face toward x = 16 is hidden");
        let n = mesh_chunk([1, 0, 0], &looks(), |c| *cells.get(&c).unwrap_or(&0), |_| 0);
        assert_eq!(quads(&n, "stone"), 5);
    }

    #[test]
    fn plants_cross_and_do_not_hide_faces() {
        let cells: HashMap<[i32; 3], u8> = [([0, 0, 0], 2), ([0, 1, 0], 3)].into();
        let m = mesh_chunk([0, 0, 0], &looks(), |c| *cells.get(&c).unwrap_or(&0), |_| 0);
        assert_eq!(quads(&m, "flower"), 2);
        assert_eq!(
            quads(&m, "stone"),
            6,
            "the top under the flower still shows"
        );
    }

    #[test]
    fn liquids_lower_their_open_top_and_hide_shared_faces() {
        let cells: HashMap<[i32; 3], u8> = [
            ([0, 0, 0], 4),
            ([1, 0, 0], 4),
            ([0, 1, 0], 4),
            ([0, -1, 0], 2),
        ]
        .into();
        let m = mesh_chunk([0, 0, 0], &looks(), |c| *cells.get(&c).unwrap_or(&0), |_| 0);
        let water = &m.iter().find(|(k, _)| k == "water").unwrap().1;
        // (0,0,0): no top (water above), no bottom (stone), no +x (water): 3 sides. (1,0,0): top,
        // bottom, 3 sides. (0,1,0): top and 4 sides.
        assert_eq!(water.indices.len() / 6, 3 + 5 + 5);
        let top = water
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(0.0, f32::max);
        assert_eq!(top, 1.0 + 8.0 / 9.0);
    }

    #[test]
    fn flowing_liquids_slope_from_their_source() {
        // A source at x = 0 and flowing levels 1 and 2 along +x.
        let cells: HashMap<[i32; 3], u8> = [([0, 0, 0], 4), ([1, 0, 0], 4), ([2, 0, 0], 4)].into();
        let states: HashMap<[i32; 3], u8> = [([1, 0, 0], 1), ([2, 0, 0], 2)].into();
        let m = mesh_chunk(
            [0, 0, 0],
            &looks(),
            |c| *cells.get(&c).unwrap_or(&0),
            |c| *states.get(&c).unwrap_or(&0),
        );
        let water = &m.iter().find(|(k, _)| k == "water").unwrap().1;
        let height_at = |x: f32| {
            water
                .vertices
                .iter()
                .filter(|v| v.normal == [0.0, 1.0, 0.0] && v.position[0] == x)
                .map(|v| v.position[1])
                .fold(0.0, f32::max)
        };
        // Shared corners take the higher side: the surface steps down cell by cell.
        assert_eq!(height_at(1.0), 8.0 / 9.0);
        assert_eq!(height_at(2.0), 7.0 / 9.0);
        assert_eq!(height_at(3.0), 6.0 / 9.0);
        // Falling liquid fills its cell.
        let fall: HashMap<[i32; 3], u8> = [([5, 5, 5], 4)].into();
        let m = mesh_chunk(
            [0, 0, 0],
            &looks(),
            |c| *fall.get(&c).unwrap_or(&0),
            |_| FALLING,
        );
        let top = m[0]
            .1
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(0.0, f32::max);
        assert_eq!(top, 6.0);
    }
}
