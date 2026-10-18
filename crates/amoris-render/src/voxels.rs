//! Block volumes in the renderer (docs/spec/voxels.md 7.2): each volume's cells as the feed sent
//! them, meshed per chunk and material into rows of the mesh pool, each drawn by an instance of
//! its own. A changed chunk is remeshed with its six neighbours (their shared faces may appear or
//! go); a new palette remeshes the volume. The instances are ordinary scene instances, so every
//! pass draws them; their ids live above the entities' and picking maps them back to the volume.

use std::collections::{BTreeMap, HashMap, HashSet};

use amoris_assets::frame::{BlockLook, InstanceUpdate, Look, Pose, VoxelUpdate};
use amoris_assets::mesh::MeshData;
use amoris_assets::voxel_mesh::{CHUNK, mesh_chunk};

use crate::splash;

/// Instance ids from here up are chunk parts, not entities (entity ids stay below 2^53).
pub const FIRST_ID: u64 = 1 << 60;

/// One volume as the renderer holds it.
#[derive(Default)]
struct Volume {
    origin: [f32; 3],
    cell_m: f32,
    blocks: Vec<BlockLook>,
    chunks: HashMap<[i32; 3], Box<[u8]>>,
    /// Each chunk's state bytes, where any is not 0 (liquids.md 6).
    states: HashMap<[i32; 3], Box<[u8]>>,
    /// The instance drawing each (chunk, material).
    parts: BTreeMap<([i32; 3], String), u64>,
}

/// A replaced chunk's key and its cells before (none: all air), for its gushes.
type Before = ([i32; 3], Option<Box<[u8]>>);

fn locate(cell: [i32; 3]) -> ([i32; 3], usize) {
    let key = cell.map(|c| c.div_euclid(CHUNK));
    let [x, y, z] = cell.map(|c| c.rem_euclid(CHUNK) as usize);
    (key, x + 16 * (z + 16 * y))
}

impl Volume {
    fn get(&self, cell: [i32; 3]) -> u8 {
        let (key, at) = locate(cell);
        self.chunks.get(&key).map_or(0, |c| c[at])
    }

    /// The lowest and highest chunk keys, per axis.
    fn chunk_box(&self) -> ([i32; 3], [i32; 3]) {
        let mut lo = [i32::MAX; 3];
        let mut hi = [i32::MIN; 3];
        for k in self.chunks.keys() {
            for a in 0..3 {
                lo[a] = lo[a].min(k[a]);
                hi[a] = hi[a].max(k[a]);
            }
        }
        (lo, hi)
    }

    fn state(&self, cell: [i32; 3]) -> u8 {
        let (key, at) = locate(cell);
        self.states.get(&key).map_or(0, |c| c[at])
    }

    /// The fog of the liquid whose surface is above `p` (world metres), if any.
    fn fog_at(&self, p: [f32; 3]) -> Option<[f32; 4]> {
        let g = [0, 1, 2].map(|a| (p[a] - self.origin[a]) / self.cell_m);
        let cell = g.map(|c| c.floor() as i32);
        let b = self.get(cell);
        let look = self.blocks.get(usize::from(b)).filter(|l| l.shape == 2)?;
        let s = self.state(cell);
        let under = self.get([cell[0], cell[1] + 1, cell[2]]) == b;
        let top = if under || s & 0b1000 != 0 {
            1.0
        } else {
            f32::from(8 - (s & 0b0111)) / 9.0
        };
        (g[1] - cell[1] as f32 <= top).then_some(look.fog?)
    }
}

impl splash::Cells for Volume {
    fn origin(&self) -> [f32; 3] {
        self.origin
    }
    fn cell_m(&self) -> f32 {
        self.cell_m
    }
    fn looks(&self) -> &[BlockLook] {
        &self.blocks
    }
    fn block(&self, c: [i32; 3]) -> u8 {
        self.get(c)
    }
    fn state(&self, c: [i32; 3]) -> u8 {
        Volume::state(self, c)
    }
    fn keys(&self) -> Vec<[i32; 3]> {
        let mut k: Vec<[i32; 3]> = self.chunks.keys().copied().collect();
        k.sort_unstable();
        k
    }
}

/// What the scene must do after volumes changed: meshes to upload (by key, replacing what each key
/// held), instances to add or move, and instances to remove; and what the splashes see change.
#[derive(Default)]
pub struct VoxelChanges {
    pub meshes: Vec<(String, MeshData)>,
    pub instances: Vec<InstanceUpdate>,
    pub removed: Vec<u64>,
    pub splash: splash::Changes,
}

/// The renderer's block volumes.
#[derive(Default)]
pub struct Voxels {
    volumes: HashMap<u64, Volume>,
    /// Each part instance's volume.
    owners: HashMap<u64, u64>,
    next: u64,
}

fn mesh_key(volume: u64, key: [i32; 3], material: &str) -> String {
    // No '.' (a mesh key with one names an asset file): the material path is in the part's look.
    let tag = material.replace('.', "_");
    format!("voxels:{volume}:{}_{}_{}:{tag}", key[0], key[1], key[2])
}

impl Voxels {
    /// Forgets every volume (a reset frame): their instances go with the scene.
    pub fn clear(&mut self) {
        self.volumes.clear();
        self.owners.clear();
    }

    /// The cell a ray from `from` along the unit `dir` meets first within `max_m`, over every
    /// volume: the cell's world corner and edge. Air and liquids let the ray through; plants stop
    /// it, as the block game targets them. Presentation only (the targeted block's outline).
    pub fn target(&self, from: [f32; 3], dir: [f32; 3], max_m: f32) -> Option<([f32; 3], f32)> {
        let mut best: Option<(f32, [f32; 3], f32)> = None;
        for v in self.volumes.values() {
            let p = [0, 1, 2].map(|a| (from[a] - v.origin[a]) / v.cell_m);
            let mut cell = p.map(|c| c.floor() as i32);
            let mut step = [0i32; 3];
            let mut t_max = [f32::INFINITY; 3];
            let mut t_delta = [f32::INFINITY; 3];
            for a in 0..3 {
                if dir[a] > 0.0 {
                    step[a] = 1;
                    t_max[a] = (cell[a] as f32 + 1.0 - p[a]) / dir[a];
                    t_delta[a] = 1.0 / dir[a];
                } else if dir[a] < 0.0 {
                    step[a] = -1;
                    t_max[a] = (p[a] - cell[a] as f32) / -dir[a];
                    t_delta[a] = -1.0 / dir[a];
                }
            }
            let limit = max_m / v.cell_m;
            let mut t = 0.0;
            while t <= limit {
                let b = v.get(cell);
                if b != 0 && v.blocks.get(usize::from(b)).is_some_and(|l| l.shape != 2) {
                    let d = t * v.cell_m;
                    if best.is_none_or(|(bd, ..)| d < bd) {
                        let corner = [0, 1, 2].map(|a| v.origin[a] + cell[a] as f32 * v.cell_m);
                        best = Some((d, corner, v.cell_m));
                    }
                    break;
                }
                let a = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
                    0
                } else if t_max[1] <= t_max[2] {
                    1
                } else {
                    2
                };
                t = t_max[a];
                cell[a] += step[a];
                t_max[a] += t_delta[a];
            }
        }
        best.map(|(_, corner, edge)| (corner, edge))
    }

    /// The fog of a liquid whose surface is above `p` in any volume (the view from under it).
    pub fn fog_at(&self, p: [f32; 3]) -> Option<[f32; 4]> {
        let mut ids: Vec<&u64> = self.volumes.keys().collect();
        ids.sort_unstable();
        ids.into_iter().find_map(|id| self.volumes[id].fog_at(p))
    }

    /// Volume `id` as the splashes read it.
    pub fn cells(&self, id: u64) -> Option<&dyn splash::Cells> {
        self.volumes.get(&id).map(|v| v as &dyn splash::Cells)
    }

    /// The volume a part instance draws, or the id itself for an entity.
    pub fn owner(&self, id: u64) -> u64 {
        self.owners.get(&id).copied().unwrap_or(id)
    }

    /// Applies the feed's volume updates and removals.
    pub fn apply(&mut self, updates: &[VoxelUpdate], removed: &[u64]) -> VoxelChanges {
        let mut out = VoxelChanges::default();
        for id in removed {
            out.splash.fresh.push(*id);
            if let Some(v) = self.volumes.remove(id) {
                for part in v.parts.values() {
                    self.owners.remove(part);
                    out.removed.push(*part);
                }
            }
        }
        for u in updates {
            let known = self.volumes.contains_key(&u.id);
            let v = self.volumes.entry(u.id).or_default();
            let moved = known && (v.origin != u.origin || v.cell_m != u.cell_m);
            (v.origin, v.cell_m) = (u.origin, u.cell_m);
            // A volume seen for the first time, moved or given new looks does not splash: its
            // regions close and its streams are found again (splashes.md 3).
            let fresh = !known || moved || u.blocks.is_some();
            if fresh {
                out.splash.fresh.push(u.id);
            }
            let mut dirty: HashSet<[i32; 3]> = HashSet::new();
            if let Some(blocks) = &u.blocks {
                v.blocks = blocks.clone();
                dirty.extend(v.chunks.keys().copied());
            }
            let mut before: Vec<Before> = Vec::new();
            for c in &u.chunks {
                if !fresh {
                    before.push((c.key, v.chunks.get(&c.key).cloned()));
                }
                out.splash.chunks.push((u.id, c.key));
                match &c.cells {
                    Some(cells) if cells.len() == 16 * 16 * 16 => {
                        v.chunks.insert(c.key, cells.clone().into_boxed_slice());
                    }
                    _ => {
                        v.chunks.remove(&c.key);
                    }
                }
                match &c.states {
                    Some(states) if states.len() == 16 * 16 * 16 && c.cells.is_some() => {
                        v.states.insert(c.key, states.clone().into_boxed_slice());
                    }
                    _ => {
                        v.states.remove(&c.key);
                    }
                }
                dirty.insert(c.key);
                for d in [
                    [1, 0, 0],
                    [-1, 0, 0],
                    [0, 1, 0],
                    [0, -1, 0],
                    [0, 0, 1],
                    [0, 0, -1],
                ] {
                    let n = [c.key[0] + d[0], c.key[1] + d[1], c.key[2] + d[2]];
                    if v.chunks.contains_key(&n) {
                        dirty.insert(n);
                    }
                }
            }
            for (key, old) in &before {
                if !v.chunks.contains_key(key) {
                    continue;
                }
                let old = |c: [i32; 3]| old.as_ref().map_or(0, |o| o[locate(c).1]);
                let gushes = splash::gushes(*key, &v.blocks, old, |c| v.get(c), |c| v.state(c));
                out.splash
                    .gushes
                    .extend(gushes.into_iter().map(|g| (u.id, g)));
            }
            let mut dirty: Vec<[i32; 3]> = dirty.into_iter().collect();
            dirty.sort_unstable();
            for key in dirty {
                let (lo, hi) = v.chunk_box();
                let outside = |c: [i32; 3]| {
                    (0..3).any(|a| c[a].div_euclid(CHUNK) < lo[a] || c[a].div_euclid(CHUNK) > hi[a])
                };
                let meshes = if v.chunks.contains_key(&key) {
                    mesh_chunk(key, &v.blocks, |c| v.get(c), |c| v.state(c), outside)
                } else {
                    Vec::new()
                };
                let fresh: HashSet<&str> = meshes.iter().map(|(m, _)| m.as_str()).collect();
                // Parts of this chunk whose material no longer appears draw nothing now.
                let stale: Vec<([i32; 3], String)> = v
                    .parts
                    .keys()
                    .filter(|(k, m)| *k == key && !fresh.contains(m.as_str()))
                    .cloned()
                    .collect();
                for part in stale {
                    if let Some(id) = v.parts.remove(&part) {
                        self.owners.remove(&id);
                        out.removed.push(id);
                        out.meshes
                            .push((mesh_key(u.id, part.0, &part.1), MeshData::default()));
                    }
                }
                for (material, mesh) in meshes {
                    let mkey = mesh_key(u.id, key, &material);
                    match v.parts.get(&(key, material.clone())) {
                        Some(_) => {}
                        None => {
                            let id = FIRST_ID + self.next;
                            self.next += 1;
                            v.parts.insert((key, material.clone()), id);
                            self.owners.insert(id, u.id);
                            out.instances
                                .push(part_instance(id, &mkey, &material, v, key));
                        }
                    }
                    out.meshes.push((mkey, mesh));
                }
            }
            if moved {
                for ((key, material), &id) in &v.parts {
                    let mkey = mesh_key(u.id, *key, material);
                    out.instances
                        .push(part_instance(id, &mkey, material, v, *key));
                }
            }
        }
        out
    }
}

/// A part's instance: placed at its chunk, scaled by the cell; a liquid's part casts no shadow
/// (liquids.md 6), as the block game's water casts none.
fn part_instance(id: u64, mesh: &str, material: &str, v: &Volume, key: [i32; 3]) -> InstanceUpdate {
    let (origin, cell_m) = (v.origin, v.cell_m);
    let liquid = v
        .blocks
        .iter()
        .any(|b| b.shape == 2 && b.faces[0] == material);
    let edge = CHUNK as f32 * cell_m;
    InstanceUpdate {
        id,
        pose: Some(Pose {
            position: [
                origin[0] + key[0] as f32 * edge,
                origin[1] + key[1] as f32 * edge,
                origin[2] + key[2] as f32 * edge,
            ],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [cell_m; 3],
        }),
        look: Some(Look {
            mesh: mesh.to_owned(),
            material: material.to_owned(),
            color: [1.0; 4],
            metallic: 0.0,
            roughness: 1.0,
            transmission: None,
            ior: None,
            emissive: [0.0; 3],
            cast_shadows: !liquid,
            visible: true,
        }),
        anim: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amoris_assets::frame::ChunkUpdate;

    fn update(chunks: Vec<ChunkUpdate>, blocks: bool) -> VoxelUpdate {
        VoxelUpdate {
            id: 7,
            origin: [0.0; 3],
            cell_m: 1.0,
            blocks: blocks.then(|| {
                vec![
                    BlockLook::default(),
                    BlockLook {
                        shape: 0,
                        faces: [
                            "b.gltf#top".into(),
                            "b.gltf#side".into(),
                            "b.gltf#side".into(),
                        ],
                        fog: None,
                        splash: None,
                    },
                ]
            }),
            chunks,
        }
    }

    fn cells(at: &[usize]) -> Option<Vec<u8>> {
        let mut c = vec![0u8; 4096];
        for &i in at {
            c[i] = 1;
        }
        Some(c)
    }

    #[test]
    fn chunks_become_parts_and_edits_remesh_them_and_their_neighbours() {
        let mut v = Voxels::default();
        let first = v.apply(
            &[update(
                vec![
                    ChunkUpdate {
                        key: [0, 0, 0],
                        cells: cells(&[15]),
                        states: None,
                    },
                    ChunkUpdate {
                        key: [1, 0, 0],
                        cells: cells(&[0]),
                        states: None,
                    },
                ],
                true,
            )],
            &[],
        );
        // Two chunks, two materials each.
        assert_eq!(first.instances.len(), 4);
        assert!(first.instances.iter().all(|i| i.id >= FIRST_ID));
        assert_eq!(v.owner(first.instances[0].id), 7);
        assert!(first.meshes.iter().all(|(k, _)| !k.contains('.')));
        // Clearing chunk 1's only block: its parts go, chunk 0's side toward it reappears.
        let edit = v.apply(
            &[update(
                vec![ChunkUpdate {
                    key: [1, 0, 0],
                    cells: None,
                    states: None,
                }],
                false,
            )],
            &[],
        );
        assert_eq!(edit.removed.len(), 2);
        assert!(
            edit.instances.is_empty(),
            "chunk 0's parts are kept, only remeshed"
        );
        let side = edit
            .meshes
            .iter()
            .find(|(k, m)| k.contains(":0_0_0:") && k.ends_with("side") && !m.indices.is_empty())
            .unwrap();
        assert_eq!(
            side.1.indices.len() / 6,
            5,
            "four sides and the bottom (also \"side\")"
        );
        let gone = v.apply(&[], &[7]);
        assert_eq!(gone.removed.len(), 2);
        assert_eq!(v.owner(first.instances[0].id), first.instances[0].id);
    }
}
