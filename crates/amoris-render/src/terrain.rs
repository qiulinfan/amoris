//! Terrains in the renderer (water.md 65.1): each terrain's heights meshed into one mesh per layer,
//! every cell going to the first layer it is steeper and lower than where the layer says, each mesh
//! drawn by an instance of its own. Along an edge between layers both meshes hold the triangles
//! there and split them pixel by pixel (`mesh`, forward.wgsl `terrain_kept`), so the ground passes
//! from one layer to the next over a few metres instead of along the cells' edges. As voxels.rs's parts, the instances are ordinary scene
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
    /// Each terrain's meshes as last made, by a hash of what it was made from, and the layers they
    /// draw. Kept through a reset: the mesh pool keeps the meshes by key, so a terrain sent again
    /// unchanged (every full frame sends every terrain whole) is not meshed again. Meshing a large
    /// terrain takes seconds in the browser, where a slow full frame could be followed by another.
    meshed: HashMap<u64, (u64, Vec<TerrainLayerView>)>,
}

fn mesh_key(id: u64, layer: u64) -> String {
    format!("terrain:{id}:{layer}")
}

/// Whether a look's mesh is a terrain layer's: its material row then takes its texture
/// coordinates from the world and its share of the ground from the mesh's (materials.rs
/// `FLAG_TERRAIN`).
pub fn is_layer_mesh(mesh: &str) -> bool {
    mesh.starts_with("terrain:")
}

/// How many cells either side of a corner its share of each layer is averaged over: the edges
/// between layers blend over about twice this many metres a cell.
const BLEND_CELLS: usize = 3;

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
            let hash = content_hash(t);
            let looks: Vec<(Option<MeshData>, TerrainLayerView)> = match self.meshed.get(&t.id) {
                Some((h, looks)) if *h == hash => looks.iter().map(|l| (None, l.clone())).collect(),
                _ => {
                    let made: Vec<_> = mesh(t)
                        .into_iter()
                        .map(|(m, look)| (Some(m), look.clone()))
                        .collect();
                    self.meshed
                        .insert(t.id, (hash, made.iter().map(|(_, l)| l.clone()).collect()));
                    made
                }
            };
            let before = self.drawn.insert(t.id, looks.len() as u64).unwrap_or(0);
            for layer in looks.len() as u64..before {
                out.removed.push(FIRST_ID + t.id * LAYERS + layer);
                out.meshes
                    .push((mesh_key(t.id, layer), MeshData::default()));
            }
            for (layer, (mesh, look)) in looks.into_iter().enumerate() {
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
                if let Some(mesh) = mesh {
                    out.meshes.push((key, mesh));
                }
            }
            self.grounds.insert(t.id, t.clone());
        }
        out
    }
}

/// What a terrain's meshes are made from: its size, cell, corner, heights and layers.
fn content_hash(t: &TerrainView) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.size.hash(&mut h);
    t.cell_m.to_bits().hash(&mut h);
    for v in t.origin {
        v.to_bits().hash(&mut h);
    }
    for v in &t.heights {
        v.to_bits().hash(&mut h);
    }
    for l in &t.layers {
        l.material.hash(&mut h);
        for c in l.color {
            c.to_bits().hash(&mut h);
        }
        l.roughness.to_bits().hash(&mut h);
        l.steeper_than.map(f32::to_bits).hash(&mut h);
        l.below_m.map(f32::to_bits).hash(&mut h);
    }
    h.finish()
}

/// A terrain's meshes in its corner's frame, one per layer that any triangle takes, with the layer:
/// smooth normals from the heights. Each corner carries, in its texture coordinates, the interval
/// of [0, 1) that its layer holds there: the layers' shares of the corner (`shares`) laid end to end
/// in layer order. A triangle goes to every layer with a share at one of its corners; the forward
/// shader draws a pixel of it in the one layer whose interpolated interval holds a noise of the
/// pixel's place, and takes the texture coordinates from the world's x and z (a metre a unit).
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
            // The texture's u is x: the tangent is the surface along +x, and its v (z) is the
            // bitangent, opposite to normal x tangent (w = -1). What compute_tangents finds, without
            // a pass over every triangle.
            let tl = (1.0 + dx * dx).sqrt();
            Vertex {
                position: [x, h(i, k), z],
                normal: [-dx / len, 1.0 / len, -dz / len],
                uv: [0.0, 1.0],
                tangent: [1.0 / tl, dx / tl, 0.0, -1.0],
            }
        })
        .collect();
    let layer_of = triangle_layers(t, &vertices);
    let shares = shares(t, &layer_of);
    let n_layers = t.layers.len();
    let mut triangles: Vec<Vec<[usize; 3]>> = vec![Vec::new(); n_layers];
    for k in 0..cz {
        for i in 0..cx {
            let a = i + nx * k;
            for (j, tri) in [[a, a + nx, a + 1], [a + 1, a + nx, a + nx + 1]]
                .into_iter()
                .enumerate()
            {
                let own = usize::from(layer_of[2 * (i + cx * k) + j]);
                for (l, tris) in triangles.iter_mut().enumerate() {
                    if l == own || tri.iter().any(|&v| shares[v * n_layers + l] > 0.0) {
                        tris.push(tri);
                    }
                }
            }
        }
    }
    triangles
        .into_iter()
        .zip(&t.layers)
        .enumerate()
        .filter(|(_, (tris, _))| !tris.is_empty())
        .map(|(l, (tris, look))| {
            // Each layer keeps only the corners its triangles use, each with its interval.
            let mut remap = vec![u32::MAX; vertices.len()];
            let mut own = Vec::new();
            let mut indices = Vec::with_capacity(tris.len() * 3);
            for tri in tris {
                for v in tri {
                    if remap[v] == u32::MAX {
                        remap[v] = own.len() as u32;
                        let row = &shares[v * n_layers..(v + 1) * n_layers];
                        let mut vertex = vertices[v];
                        vertex.uv = interval(row, l);
                        own.push(vertex);
                    }
                    indices.push(remap[v]);
                }
            }
            (MeshData::new("terrain", own, indices), look)
        })
        .collect()
}

/// The interval of [0, 1) that layer `l` holds at a corner with these shares: the shares before it
/// laid end to end, the last layer with a share reaching exactly 1, so the layers' intervals at a
/// corner meet exactly and cover [0, 1) whatever the rounding of the sums.
fn interval(shares: &[f32], l: usize) -> [f32; 2] {
    let last = shares.iter().rposition(|&w| w > 0.0).unwrap_or(0);
    let end = |l: usize| {
        if l >= last {
            1.0
        } else {
            shares[..=l].iter().sum::<f32>()
        }
    };
    [if l == 0 { 0.0 } else { end(l - 1) }, end(l)]
}

/// Each triangle's layer (two a cell, the cell's lower-left then its upper-right, cells along x
/// then z), by its own slope and height, both moved by a noise of its place in the world, so an
/// edge wanders as ground does instead of following a contour line or a slope's crest
/// (presentation only: the layers' rules stay what the terrain says).
fn triangle_layers(t: &TerrainView, vertices: &[Vertex]) -> Vec<u8> {
    let [cx, cz] = t.size.map(|n| n as usize);
    let nx = cx + 1;
    let layer_of = |tri: [usize; 3]| {
        let p = tri.map(|v| vertices[v].position);
        let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
        let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
        let cx = (p[0][0] + p[1][0] + p[2][0]) / 3.0;
        let cz = (p[0][2] + p[1][2] + p[2][2]) / 3.0;
        let (wx, wz) = (t.origin[0] + cx, t.origin[2] + cz);
        let slope = 1.0 - (n[1] / len).abs() + 0.05 * edge_noise(wx, wz);
        let height = (p[0][1] + p[1][1] + p[2][1]) / 3.0 + 2.0 * edge_noise(wz + 71.0, wx - 13.0);
        t.layers
            .iter()
            .position(|l| {
                l.steeper_than.is_none_or(|s| slope > s) && l.below_m.is_none_or(|b| height < b)
            })
            .unwrap_or(t.layers.len() - 1) as u8
    };
    let mut out = Vec::with_capacity(2 * cx * cz);
    for k in 0..cz {
        for i in 0..cx {
            let a = i + nx * k;
            out.push(layer_of([a, a + nx, a + 1]));
            out.push(layer_of([a + 1, a + nx, a + nx + 1]));
        }
    }
    out
}

/// Each corner's share of each layer (`corner * layers + layer`): the part of the triangles within
/// `BLEND_CELLS` cells of it that the layer takes, from summed counts per layer.
fn shares(t: &TerrainView, layer_of: &[u8]) -> Vec<f32> {
    let [cx, cz] = t.size.map(|n| n as usize);
    let (nx, nz) = (cx + 1, cz + 1);
    let layers = t.layers.len();
    // sums[l][(i, k)]: layer l's triangles in the cells below i along x and below k along z.
    let mut sums = vec![vec![0u32; nx * nz]; layers];
    for (l, sum) in sums.iter_mut().enumerate() {
        for k in 0..cz {
            for i in 0..cx {
                let c = 2 * (i + cx * k);
                let here = u32::from(usize::from(layer_of[c]) == l)
                    + u32::from(usize::from(layer_of[c + 1]) == l);
                sum[(i + 1) + nx * (k + 1)] =
                    here + sum[i + nx * (k + 1)] + sum[(i + 1) + nx * k] - sum[i + nx * k];
            }
        }
    }
    let mut out = vec![0.0f32; nx * nz * layers];
    for k in 0..nz {
        let (k0, k1) = (k.saturating_sub(BLEND_CELLS), (k + BLEND_CELLS).min(cz));
        for i in 0..nx {
            let (i0, i1) = (i.saturating_sub(BLEND_CELLS), (i + BLEND_CELLS).min(cx));
            let total = (2 * (i1 - i0) * (k1 - k0)) as f32;
            for (l, sum) in sums.iter().enumerate() {
                let n =
                    sum[i1 + nx * k1] + sum[i0 + nx * k0] - sum[i0 + nx * k1] - sum[i1 + nx * k0];
                out[(i + nx * k) * layers + l] = n as f32 / total;
            }
        }
    }
    out
}

/// Value noise in -1..1 over the world's x and z: two octaves of smoothly blended lattice values
/// (16 m and 6 m apart) from an integer hash, the same on every platform. Coarse on purpose: a
/// finer octave speckles an edge cell by cell.
fn edge_noise(x: f32, z: f32) -> f32 {
    fn hash(i: i32, k: i32) -> f32 {
        let mut h = (i as u32).wrapping_mul(0x8da6_b343) ^ (k as u32).wrapping_mul(0xd816_3841);
        h = (h ^ (h >> 15)).wrapping_mul(0x2c1b_3c6d);
        h ^= h >> 12;
        (h & 0xffff) as f32 / 32767.5 - 1.0
    }
    fn octave(x: f32, z: f32) -> f32 {
        let (fx, fz) = (x.floor(), z.floor());
        let (u, v) = (x - fx, z - fz);
        let (u, v) = (u * u * (3.0 - 2.0 * u), v * v * (3.0 - 2.0 * v));
        let (i, k) = (fx as i32, fz as i32);
        let a = hash(i, k) + (hash(i + 1, k) - hash(i, k)) * u;
        let b = hash(i, k + 1) + (hash(i + 1, k + 1) - hash(i, k + 1)) * u;
        a + (b - a) * v
    }
    0.75 * octave(x / 16.0, z / 16.0) + 0.25 * octave(x / 6.0 + 17.0, z / 6.0 - 5.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terrains_analytic_tangents_are_what_compute_tangents_finds() {
        let (nx, nz) = (9usize, 7usize);
        let heights = (0..nx * nz)
            .map(|n| ((n % nx) as f32 * 0.7).sin() * 1.3 + ((n / nx) as f32 * 0.5).cos() * 0.8)
            .collect();
        let t = TerrainView {
            id: 1,
            origin: [0.0; 3],
            cell_m: 1.5,
            size: [nx as u32 - 1, nz as u32 - 1],
            heights,
            layers: vec![layer(None, None)],
        };
        let (mut m, _) = mesh(&t).into_iter().next().unwrap();
        let analytic: Vec<[f32; 4]> = m.vertices.iter().map(|v| v.tangent).collect();
        // The shader's texture coordinates are the world's x and z (the mesh's uv is the layer's
        // interval).
        for v in &mut m.vertices {
            v.uv = [v.position[0], v.position[2]];
        }
        m.compute_tangents();
        for (v, a) in m.vertices.iter().zip(&analytic) {
            let c = v.tangent;
            // Interior and edge corners alike: the triangle-averaged tangent leans with the
            // neighbouring cells, so the directions agree closely rather than exactly.
            let dot = c[0] * a[0] + c[1] * a[1] + c[2] * a[2];
            assert!(dot > 0.97, "tangent {a:?} against {c:?}");
            assert_eq!(c[3], a[3], "handedness");
        }
    }

    #[test]
    fn the_layers_intervals_at_a_corner_cover_zero_to_one_once() {
        // A slope between a shelf and a floor, three layers sharing the corners near the edges.
        let (cx, cz) = (16u32, 6u32);
        let heights = (0..(cx + 1) * (cz + 1))
            .map(|n| ((n % (cx + 1)) as f32 - 8.0).clamp(-2.0, 2.0) * 1.5)
            .collect();
        let view = TerrainView {
            id: 3,
            origin: [0.0; 3],
            cell_m: 1.0,
            size: [cx, cz],
            heights,
            layers: vec![
                layer(Some(0.5), None),
                layer(None, Some(0.0)),
                layer(None, None),
            ],
        };
        // Every corner of every layer's mesh, by its place: the intervals the layers hold there.
        let mut at: HashMap<[u32; 2], Vec<[f32; 2]>> = HashMap::new();
        for (m, _) in mesh(&view) {
            for v in &m.vertices {
                at.entry([v.position[0].to_bits(), v.position[2].to_bits()])
                    .or_default()
                    .push(v.uv);
            }
        }
        let shared = at.values().filter(|i| i.len() > 1).count();
        assert!(shared > 0, "no corner is shared");
        for mut intervals in at.into_values() {
            intervals.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
            let mut end = 0.0f32;
            for [lo, hi] in intervals {
                assert!(lo <= hi);
                if hi > lo {
                    assert_eq!(lo, end, "a gap or an overlap");
                    end = hi;
                }
            }
            assert_eq!(end, 1.0);
        }
    }

    #[test]
    fn an_unchanged_terrain_sent_again_is_not_meshed_again() {
        let t = TerrainView {
            id: 4,
            origin: [0.0; 3],
            cell_m: 1.0,
            size: [4, 4],
            heights: vec![0.5; 25],
            layers: vec![layer(None, None)],
        };
        let mut terrains = Terrains::default();
        let first = terrains.apply(std::slice::from_ref(&t), &[]);
        assert_eq!(first.meshes.len(), 1);
        terrains.clear();
        let again = terrains.apply(std::slice::from_ref(&t), &[]);
        assert!(
            again.meshes.is_empty(),
            "meshed again: {}",
            again.meshes.len()
        );
        assert_eq!(again.instances.len(), first.instances.len());
        let mut moved = t.clone();
        moved.heights[3] = 1.0;
        assert_eq!(terrains.apply(&[moved], &[]).meshes.len(), 1);
    }

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
        let vertices: Vec<Vertex> = (0..(cx + 1) * (cz + 1))
            .map(|n| Vertex {
                position: [
                    (n % (cx + 1)) as f32,
                    view.heights[n as usize],
                    (n / (cx + 1)) as f32,
                ],
                ..Default::default()
            })
            .collect();
        let mut own = [0usize; 3];
        for l in triangle_layers(&view, &vertices) {
            own[usize::from(l)] += 1;
        }
        assert_eq!(own, [2 * 4, 2 * 4 * 4, 2 * 3 * 4], "wall, floor, shelf");
        // Near the edges the layers share triangles; every triangle is in some layer.
        let tris: Vec<usize> = meshes.iter().map(|(m, _)| m.triangles()).collect();
        assert!(tris.iter().sum::<usize>() > 2 * 8 * 4);
        assert!(tris.iter().zip(own).all(|(&n, o)| n >= o), "{tris:?}");
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
