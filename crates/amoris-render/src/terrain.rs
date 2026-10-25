//! Terrains in the renderer (water.md 65.1): each terrain's heights meshed into one mesh per layer,
//! every cell going to the first layer it is steeper and lower than where the layer says, each mesh
//! drawn by an instance of its own. As voxels.rs's parts, the instances are ordinary scene
//! instances whose ids live above the entities', so every pass draws them and picking maps them
//! back to the terrain.

use std::collections::HashMap;

use amoris_assets::frame::{InstanceUpdate, Look, Pose, TerrainLayerView, TerrainView};
use amoris_assets::mesh::{MeshData, Vertex};

/// Instance ids from here up, below the voxels' parts, are terrain layers: a terrain's layer `n`
/// is `FIRST_ID + id * LAYERS + n` (entity ids stay below 2^53).
pub const FIRST_ID: u64 = 1 << 59;
/// The most layers a terrain draws.
pub const LAYERS: u64 = 8;

/// What a batch of terrain updates changes: meshes to upload (an empty one drops its slot),
/// instances to add or move, instances gone.
#[derive(Default)]
pub struct TerrainChanges {
    pub meshes: Vec<(String, MeshData)>,
    pub instances: Vec<InstanceUpdate>,
    pub removed: Vec<u64>,
}

/// The renderer's terrains: how many layer instances each draws, and its ground for picking.
#[derive(Default)]
pub struct Terrains {
    drawn: HashMap<u64, u64>,
    grounds: HashMap<u64, TerrainView>,
}

fn mesh_key(id: u64, layer: u64) -> String {
    format!("terrain:{id}:{layer}")
}

impl Terrains {
    /// The terrain a layer instance draws, or the id itself for an entity.
    pub fn owner(&self, id: u64) -> u64 {
        if (FIRST_ID..crate::voxels::FIRST_ID).contains(&id) {
            (id - FIRST_ID) / LAYERS
        } else {
            id
        }
    }

    pub fn clear(&mut self) {
        self.drawn.clear();
        self.grounds.clear();
    }

    /// Where a ray from `from` along the unit `dir` first meets terrain `id`'s ground between
    /// `t0` and `t1` (its parts' boxes), as the distance along it: marched half a cell at a time,
    /// then halved down to a millimetre.
    pub fn hit(&self, id: u64, from: [f32; 3], dir: [f32; 3], t0: f32, t1: f32) -> Option<f32> {
        let g = self.grounds.get(&id)?;
        let (nx, nz) = (g.size[0] as usize + 1, g.size[1] as usize + 1);
        let height = |x: f32, z: f32| {
            let gx = ((x - g.origin[0]) / g.cell_m).clamp(0.0, (nx - 1) as f32);
            let gz = ((z - g.origin[2]) / g.cell_m).clamp(0.0, (nz - 1) as f32);
            let (i, k) = ((gx as usize).min(nx - 2), (gz as usize).min(nz - 2));
            let (tx, tz) = (gx - i as f32, gz - k as f32);
            let at = |i: usize, k: usize| g.heights[i + nx * k];
            let a = at(i, k) + (at(i + 1, k) - at(i, k)) * tx;
            let b = at(i, k + 1) + (at(i + 1, k + 1) - at(i, k + 1)) * tx;
            g.origin[1] + a + (b - a) * tz
        };
        let above =
            |t: f32| from[1] + dir[1] * t - height(from[0] + dir[0] * t, from[2] + dir[2] * t);
        let step = (0.5 * g.cell_m).max(1e-3);
        let (mut lo, mut t) = (t0.max(0.0), t0.max(0.0));
        if above(lo) <= 0.0 {
            return Some(lo);
        }
        while t < t1 {
            t = (t + step).min(t1);
            if above(t) <= 0.0 {
                let mut hi = t;
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    if above(mid) > 0.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                return Some(hi);
            }
            lo = t;
        }
        None
    }

    /// Whether `id` is a terrain layer's instance.
    pub fn is_layer(id: u64) -> bool {
        (FIRST_ID..crate::voxels::FIRST_ID).contains(&id)
    }

    /// Applies the feed's terrains, each sent whole, and removals.
    pub fn apply(&mut self, updates: &[TerrainView], removed: &[u64]) -> TerrainChanges {
        let mut out = TerrainChanges::default();
        for id in removed {
            self.grounds.remove(id);
            for layer in 0..self.drawn.remove(id).unwrap_or(0) {
                out.removed.push(FIRST_ID + id * LAYERS + layer);
                out.meshes.push((mesh_key(*id, layer), MeshData::default()));
            }
        }
        for t in updates {
            let meshes = mesh(t);
            let before = self.drawn.insert(t.id, meshes.len() as u64).unwrap_or(0);
            for layer in meshes.len() as u64..before {
                out.removed.push(FIRST_ID + t.id * LAYERS + layer);
                out.meshes
                    .push((mesh_key(t.id, layer), MeshData::default()));
            }
            for (layer, (mesh, look)) in meshes.into_iter().enumerate() {
                let layer = layer as u64;
                let key = mesh_key(t.id, layer);
                out.instances.push(InstanceUpdate {
                    id: FIRST_ID + t.id * LAYERS + layer,
                    pose: Some(Pose {
                        position: t.origin,
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0; 3],
                    }),
                    look: Some(Look {
                        mesh: key.clone(),
                        material: look.material.clone(),
                        color: look.color,
                        metallic: 0.0,
                        roughness: look.roughness,
                        transmission: None,
                        ior: None,
                        emissive: [0.0; 3],
                        cast_shadows: true,
                        visible: true,
                    }),
                    anim: None,
                });
                out.meshes.push((key, mesh));
            }
            self.grounds.insert(t.id, t.clone());
        }
        out
    }
}

/// A terrain's meshes in its corner's frame, one per layer that any triangle takes, with the layer:
/// smooth normals from the heights, texture coordinates a unit a metre.
pub fn mesh(t: &TerrainView) -> Vec<(MeshData, &TerrainLayerView)> {
    let [cx, cz] = t.size.map(|n| n as usize);
    let (nx, nz) = (cx + 1, cz + 1);
    if t.heights.len() != nx * nz || t.layers.is_empty() || t.cell_m <= 0.0 {
        return Vec::new();
    }
    let h = |i: usize, k: usize| t.heights[i.min(nx - 1) + nx * k.min(nz - 1)];
    let cell = t.cell_m;
    let vertices: Vec<Vertex> = (0..nx * nz)
        .map(|n| {
            let (i, k) = (n % nx, n / nx);
            // Central differences, one-sided at the edges.
            let (il, ir) = (i.saturating_sub(1), (i + 1).min(nx - 1));
            let (kl, kr) = (k.saturating_sub(1), (k + 1).min(nz - 1));
            let dx = (h(ir, k) - h(il, k)) / ((ir - il) as f32 * cell);
            let dz = (h(i, kr) - h(i, kl)) / ((kr - kl) as f32 * cell);
            let len = (dx * dx + 1.0 + dz * dz).sqrt();
            let (x, z) = (i as f32 * cell, k as f32 * cell);
            Vertex {
                position: [x, h(i, k), z],
                normal: [-dx / len, 1.0 / len, -dz / len],
                uv: [x, z],
                tangent: [1.0, 0.0, 0.0, 1.0],
            }
        })
        .collect();
    // A cell takes one layer, by its mean slope and height: a layer's edge steps along the cells
    // rather than zigzagging between a cell's two triangles.
    let mut triangles: Vec<Vec<[usize; 3]>> = vec![Vec::new(); t.layers.len()];
    for k in 0..cz {
        for i in 0..cx {
            let a = i + nx * k;
            let y = [a, a + 1, a + nx, a + nx + 1].map(|v| vertices[v].position[1]);
            let dx = (y[1] + y[3] - y[0] - y[2]) / (2.0 * cell);
            let dz = (y[2] + y[3] - y[0] - y[1]) / (2.0 * cell);
            let slope = 1.0 - 1.0 / (1.0 + dx * dx + dz * dz).sqrt();
            let height = (y[0] + y[1] + y[2] + y[3]) / 4.0;
            let layer = t
                .layers
                .iter()
                .position(|l| {
                    l.steeper_than.is_none_or(|s| slope > s) && l.below_m.is_none_or(|b| height < b)
                })
                .unwrap_or(t.layers.len() - 1);
            triangles[layer].push([a, a + nx, a + 1]);
            triangles[layer].push([a + 1, a + nx, a + nx + 1]);
        }
    }
    triangles
        .into_iter()
        .zip(&t.layers)
        .filter(|(tris, _)| !tris.is_empty())
        .map(|(tris, look)| {
            // Each layer keeps only the corners its triangles use.
            let mut remap = vec![u32::MAX; vertices.len()];
            let mut own = Vec::new();
            let mut indices = Vec::with_capacity(tris.len() * 3);
            for tri in tris {
                for v in tri {
                    if remap[v] == u32::MAX {
                        remap[v] = own.len() as u32;
                        own.push(vertices[v]);
                    }
                    indices.push(remap[v]);
                }
            }
            let mut m = MeshData::new("terrain", own, indices);
            m.compute_tangents();
            (m, look)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(steeper_than: Option<f32>, below_m: Option<f32>) -> TerrainLayerView {
        TerrainLayerView {
            material: String::new(),
            color: [1.0; 4],
            roughness: 1.0,
            steeper_than,
            below_m,
        }
    }

    #[test]
    fn a_terrain_meshes_into_its_layers_and_its_instances_map_back() {
        // A flat shelf 2 m up that drops as a wall to a floor at 0: rock on the wall, sand on the
        // floor, grass on the shelf.
        let (cx, cz) = (8u32, 4u32);
        let heights = (0..(cx + 1) * (cz + 1))
            .map(|n| if n % (cx + 1) < 4 { 2.0 } else { 0.0 })
            .collect();
        let view = TerrainView {
            id: 42,
            origin: [10.0, 1.0, 5.0],
            cell_m: 1.0,
            size: [cx, cz],
            heights,
            layers: vec![
                layer(Some(0.5), None),
                layer(None, Some(0.5)),
                layer(None, None),
            ],
        };
        let meshes = mesh(&view);
        assert_eq!(meshes.len(), 3);
        let tris: Vec<usize> = meshes.iter().map(|(m, _)| m.triangles()).collect();
        assert_eq!(tris.iter().sum::<usize>(), 2 * 8 * 4);
        assert_eq!(
            tris,
            vec![2 * 4, 2 * 4 * 4, 2 * 3 * 4],
            "wall, floor, shelf"
        );
        let mut t = Terrains::default();
        let c = t.apply(std::slice::from_ref(&view), &[]);
        assert_eq!(c.instances.len(), 3);
        assert!(c.instances.iter().all(|i| t.owner(i.id) == 42));
        assert_eq!(t.owner(7), 7);
        // A click finds the ground under it, not the top of the terrain's box: straight down onto
        // the floor (0 m over the corner at 1 m) from 10 m, and onto the shelf at a slant.
        let t_floor = t
            .hit(42, [16.0, 11.0, 7.0], [0.0, -1.0, 0.0], 0.0, 20.0)
            .unwrap();
        assert!((t_floor - 10.0).abs() < 1e-3, "{t_floor}");
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let t_shelf = t
            .hit(42, [10.0, 5.0, 6.0], [s, -s, 0.0], 0.0, 20.0)
            .unwrap();
        assert!((5.0 - s * t_shelf - 3.0).abs() < 1e-3, "{t_shelf}");
        // From higher it passes over the shelf and the wall to the floor.
        let t_over = t
            .hit(42, [10.0, 9.0, 6.0], [s, -s, 0.0], 0.0, 20.0)
            .unwrap();
        assert!((9.0 - s * t_over - 1.0).abs() < 1e-3, "{t_over}");
        assert!(
            t.hit(42, [16.0, 11.0, 7.0], [0.0, 1.0, 0.0], 0.0, 20.0)
                .is_none()
        );
        let c = t.apply(&[], &[42]);
        assert_eq!(c.removed.len(), 3);
        assert!(
            t.hit(42, [16.0, 11.0, 7.0], [0.0, -1.0, 0.0], 0.0, 20.0)
                .is_none()
        );
    }
}
