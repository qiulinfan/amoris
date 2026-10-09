//! Every mesh in one vertex buffer and one index buffer (charter 4.4: GPU-driven drawing needs all
//! geometry reachable from one draw call), with a table of per-mesh draw ranges and bounds the
//! culling pass reads. Buffers grow by doubling; growing bumps `generation` so bind groups and
//! pipelines that hold the old buffers are rebuilt.
//!
//! A mesh with levels of detail (`MeshData::lods`, docs/spec/lod.md) takes one row of the table
//! per level: its own row (level 0, the id instances name) followed by a row per coarser level with
//! that level's index range over the same vertices and its error. A level row is a mesh like any
//! other to the draw batches; only the culling pass, which picks an instance's level, knows that
//! it stands for its mesh.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use pocket_assets::mesh::{MeshData, Vertex};

/// One mesh's draw range and bounds (matches `MeshInfo` in common.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct MeshInfo {
    pub center: [f32; 3],
    pub radius: f32,
    pub index_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    /// Where this mesh's region starts in each view's visible list (set by the scene).
    pub batch_offset: u32,
    /// The bounding box's centre and half extents (occlusion culling).
    pub box_center: [f32; 3],
    /// On a mesh's own row, its levels: the count (the full mesh included) in the top 8 bits and
    /// the row of level 1 in the low 24 (`lods >> 24 < 2`: no coarser levels). 0 on level rows.
    pub lods: u32,
    pub box_half: [f32; 3],
    /// The level's geometric error in the mesh's units (0 for the full mesh).
    pub lod_error: f32,
}

impl MeshInfo {
    /// The mesh's levels, the full mesh included (1 without coarser levels).
    pub fn levels(&self) -> u32 {
        (self.lods >> 24).max(1)
    }

    /// The row of level `level` (1 or more) of this mesh.
    pub fn level_row(&self, level: u32) -> u32 {
        (self.lods & 0x00ff_ffff) + level - 1
    }
}

/// Packs a level count and the row of level 1 into [`MeshInfo::lods`].
fn pack_lods(levels: usize, first: usize) -> u32 {
    if levels < 2 {
        return 0;
    }
    assert!(
        first < 1 << 24 && levels < 256,
        "mesh table too large for LOD rows"
    );
    ((levels as u32) << 24) | first as u32
}

/// A box's centre and half extents from its corners.
fn center_half(lo: [f32; 3], hi: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    (
        std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5),
        std::array::from_fn(|i| ((hi[i] - lo[i]) * 0.5).max(0.0)),
    )
}

/// How far a skinned copy's bounds reach beyond its bind pose's: its sphere's radius grows by this
/// (an assumption about how far animations move vertices, not a guarantee).
pub const SKINNED_GROW: f32 = 2.0;

/// The culling bounds of a mesh whose vertices move (a skinned copy) from its source's: the
/// source's sphere grown by `grow`, and as the occlusion box the cube around that sphere. A pose
/// may reach anywhere the grown sphere does (an arm swung forward goes far along a body's thin
/// axis), so the box must hold the sphere for occlusion culling to assume no more than frustum
/// culling does; the source's box grown per axis does not (docs/spec/occlusion.md 5).
pub fn dynamic_bounds(src: &MeshInfo, grow: f32) -> MeshInfo {
    let radius = src.radius * grow;
    MeshInfo {
        radius,
        box_center: src.center,
        box_half: [radius; 3],
        lods: 0,
        ..*src
    }
}

pub struct MeshPool {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub info_buffer: wgpu::Buffer,
    pub infos: Vec<MeshInfo>,
    names: Vec<String>,
    /// Local-space bounding boxes (min, max), for ray picking on the CPU.
    pub boxes: Vec<([f32; 3], [f32; 3])>,
    /// Each mesh's vertex count, and whether the GPU writes its vertices (skinning): what
    /// ray-traced shadows need to build acceleration structures (rt_shadows.rs).
    pub vertex_counts: Vec<u32>,
    pub dynamic: Vec<bool>,
    /// Per row, the row whose instances it draws: itself for a mesh's own row, the mesh's row for a
    /// level row (the draw batches size a level's regions by its mesh's instances).
    pub owner: Vec<u32>,
    by_key: HashMap<String, u32>,
    vertex_len: u64,
    index_len: u64,
    info_dirty: bool,
    /// Bumped whenever a buffer is replaced.
    pub generation: u64,
}

const VERTEX: u64 = std::mem::size_of::<Vertex>() as u64;

fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    // Geometry also feeds acceleration-structure builds, and masked casters' texture lookups, on
    // a device with ray queries, which only ray-traced shadows request (rt_shadows.rs).
    let geometry = usage.intersects(wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX);
    let blas_input = if geometry
        && device
            .features()
            .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
    {
        wgpu::BufferUsages::BLAS_INPUT | wgpu::BufferUsages::STORAGE
    } else {
        wgpu::BufferUsages::empty()
    };
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: usage | blas_input | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

impl MeshPool {
    pub fn new(device: &wgpu::Device) -> MeshPool {
        MeshPool {
            // STORAGE: the skinning pass writes skinned parts' vertices in place.
            vertices: buffer(
                device,
                "mesh vertices",
                1 << 20,
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE,
            ),
            indices: buffer(device, "mesh indices", 1 << 20, wgpu::BufferUsages::INDEX),
            info_buffer: buffer(device, "mesh infos", 64 * 32, wgpu::BufferUsages::STORAGE),
            infos: Vec::new(),
            names: Vec::new(),
            boxes: Vec::new(),
            vertex_counts: Vec::new(),
            dynamic: Vec::new(),
            owner: Vec::new(),
            by_key: HashMap::new(),
            vertex_len: 0,
            index_len: 0,
            info_dirty: false,
            generation: 0,
        }
    }

    pub fn get(&self, key: &str) -> Option<u32> {
        self.by_key.get(key).copied()
    }

    pub fn name(&self, id: u32) -> &str {
        self.names.get(id as usize).map_or("", String::as_str)
    }

    pub fn len(&self) -> usize {
        self.infos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.infos.is_empty()
    }

    /// Whether row `id` is a coarser level of another mesh.
    pub fn is_level(&self, id: u32) -> bool {
        self.owner.get(id as usize).is_some_and(|&o| o != id)
    }

    /// The level of detail row `id` draws (0: a mesh's own row).
    pub fn level_of(&self, id: u32) -> u32 {
        match self.owner.get(id as usize) {
            Some(&o) if o != id => id - self.infos[o as usize].level_row(1) + 1,
            _ => 0,
        }
    }

    fn grow(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        old: &wgpu::Buffer,
        used: u64,
        need: u64,
        label: &str,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        let mut size = old.size();
        while size < need {
            size *= 2;
        }
        let new = buffer(device, label, size, usage);
        if used > 0 {
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mesh pool growth"),
            });
            enc.copy_buffer_to_buffer(old, 0, &new, 0, used);
            queue.submit([enc.finish()]);
        }
        new
    }

    /// Uploads `mesh` under `key` (a key already present returns its id).
    pub fn add(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &str,
        mesh: &MeshData,
    ) -> u32 {
        if let Some(id) = self.by_key.get(key) {
            return *id;
        }
        let vbytes = mesh.vertices.len() as u64 * VERTEX;
        let levels = 1 + mesh.lods.len().min(pocket_assets::lod::MAX_LEVELS - 1);
        let lists: Vec<&[u32]> = std::iter::once(mesh.indices.as_slice())
            .chain(mesh.lods.iter().map(|l| l.indices.as_slice()))
            .take(levels)
            .collect();
        let ibytes = lists.iter().map(|l| l.len() as u64 * 4).sum::<u64>();
        if self.vertex_len + vbytes > self.vertices.size() {
            self.vertices = Self::grow(
                device,
                queue,
                &self.vertices,
                self.vertex_len,
                self.vertex_len + vbytes,
                "mesh vertices",
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE,
            );
            self.generation += 1;
        }
        // Index data must stay 4-byte aligned; writes must be multiples of 4 bytes.
        if self.index_len + ibytes > self.indices.size() {
            self.indices = Self::grow(
                device,
                queue,
                &self.indices,
                self.index_len,
                self.index_len + ibytes,
                "mesh indices",
                wgpu::BufferUsages::INDEX,
            );
            self.generation += 1;
        }
        let base_vertex = (self.vertex_len / VERTEX) as i32;
        let first_index = (self.index_len / 4) as u32;
        queue.write_buffer(
            &self.vertices,
            self.vertex_len,
            bytemuck::cast_slice(&mesh.vertices),
        );
        let all: Vec<u32> = lists.concat();
        queue.write_buffer(&self.indices, self.index_len, bytemuck::cast_slice(&all));
        self.vertex_len += vbytes;
        self.index_len += ibytes;
        let id = self.infos.len() as u32;
        let (box_center, box_half) = center_half(mesh.bounds.min, mesh.bounds.max);
        let mut first = first_index;
        for (level, list) in lists.iter().enumerate() {
            self.infos.push(MeshInfo {
                center: mesh.bounds.center,
                radius: mesh.bounds.radius,
                index_count: list.len() as u32,
                first_index: first,
                base_vertex,
                batch_offset: 0,
                box_center,
                lods: if level == 0 {
                    pack_lods(levels, id as usize + 1)
                } else {
                    0
                },
                box_half,
                lod_error: if level == 0 {
                    0.0
                } else {
                    mesh.lods[level - 1].error
                },
            });
            first += list.len() as u32;
            self.names.push(if level == 0 {
                key.to_owned()
            } else {
                format!("{key}@lod{level}")
            });
            self.boxes.push((mesh.bounds.min, mesh.bounds.max));
            self.vertex_counts.push(mesh.vertices.len() as u32);
            self.dynamic.push(false);
            self.owner.push(id);
        }
        self.by_key.insert(key.to_owned(), id);
        self.info_dirty = true;
        id
    }

    /// A mesh entry with its own `count` vertices (written by the GPU, e.g. skinning) drawn with the
    /// indices of mesh `indices_of`; its culling bounds are [`dynamic_bounds`] of the source's.
    pub fn add_dynamic(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &str,
        indices_of: u32,
        count: u32,
        grow: f32,
    ) -> Option<u32> {
        if let Some(id) = self.by_key.get(key) {
            return Some(*id);
        }
        let src = *self.infos.get(indices_of as usize)?;
        let src_box = self.boxes.get(indices_of as usize).copied()?;
        let vbytes = u64::from(count) * VERTEX;
        if self.vertex_len + vbytes > self.vertices.size() {
            self.vertices = Self::grow(
                device,
                queue,
                &self.vertices,
                self.vertex_len,
                self.vertex_len + vbytes,
                "mesh vertices",
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE,
            );
            self.generation += 1;
        }
        let base_vertex = (self.vertex_len / VERTEX) as i32;
        self.vertex_len += vbytes;
        let id = self.infos.len() as u32;
        let (lo, hi) = src_box;
        let c = [
            (lo[0] + hi[0]) * 0.5,
            (lo[1] + hi[1]) * 0.5,
            (lo[2] + hi[2]) * 0.5,
        ];
        let g = |i: usize, v: [f32; 3]| c[i] + (v[i] - c[i]) * grow;
        // The copy's levels: the source's index ranges over the copy's own vertices.
        let levels = src.levels();
        for level in 0..levels {
            let from = if level == 0 {
                src
            } else {
                self.infos[src.level_row(level) as usize]
            };
            self.infos.push(MeshInfo {
                base_vertex,
                batch_offset: 0,
                lods: if level == 0 {
                    pack_lods(levels as usize, id as usize + 1)
                } else {
                    0
                },
                lod_error: from.lod_error,
                ..dynamic_bounds(&from, grow)
            });
            // Ray picking keeps the source's box grown per axis.
            self.boxes.push((
                [g(0, lo), g(1, lo), g(2, lo)],
                [g(0, hi), g(1, hi), g(2, hi)],
            ));
            self.names.push(if level == 0 {
                key.to_owned()
            } else {
                format!("{key}@lod{level}")
            });
            self.vertex_counts.push(count);
            self.dynamic.push(true);
            self.owner.push(id);
        }
        self.by_key.insert(key.to_owned(), id);
        self.info_dirty = true;
        Some(id)
    }

    /// Sets each mesh's region start in the visible lists.
    pub fn set_batch_offsets(&mut self, offsets: &[u32]) {
        for (info, &o) in self.infos.iter_mut().zip(offsets) {
            if info.batch_offset != o {
                info.batch_offset = o;
                self.info_dirty = true;
            }
        }
    }

    /// Writes the info table if it changed; returns whether its buffer was replaced.
    pub fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        if !self.info_dirty {
            return false;
        }
        self.info_dirty = false;
        let bytes = (self.infos.len() * std::mem::size_of::<MeshInfo>()) as u64;
        let mut replaced = false;
        if bytes > self.info_buffer.size() {
            self.info_buffer = buffer(
                device,
                "mesh infos",
                bytes.next_power_of_two(),
                wgpu::BufferUsages::STORAGE,
            );
            self.generation += 1;
            replaced = true;
        }
        queue.write_buffer(&self.info_buffer, 0, bytemuck::cast_slice(&self.infos));
        replaced
    }

    pub fn total_bytes(&self) -> u64 {
        self.vertex_len + self.index_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skinned_copys_occlusion_box_holds_its_grown_sphere() {
        // samples/anim hero.0's bind pose: a thin box (0.135 m half depth) in a 0.573 m sphere.
        let src = MeshInfo {
            center: [0.01, 1.02, 0.03],
            radius: 0.573,
            index_count: 3,
            box_center: [0.01, 1.02, 0.03],
            box_half: [0.315, 0.475, 0.135],
            ..MeshInfo::default()
        };
        let d = dynamic_bounds(&src, SKINNED_GROW);
        assert_eq!(d.radius, 0.573 * SKINNED_GROW);
        assert_eq!((d.center, d.index_count), (src.center, src.index_count));
        // Every point of the sphere frustum culling assumes is in the box occlusion culling tests:
        // an arm swung forward 0.61 m along the thin axis, inside the sphere, is inside the box.
        for i in 0..3 {
            assert_eq!(d.box_center[i], d.center[i]);
            assert!(
                d.box_half[i] >= d.radius,
                "{:?} against {}",
                d.box_half,
                d.radius
            );
        }
    }
}
