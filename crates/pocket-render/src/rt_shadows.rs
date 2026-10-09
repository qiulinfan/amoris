//! Ray-traced sun shadows (charter 4.4, Pioneer 2026-10-09; docs/spec/rt-shadows.md): opt-in with
//! `POCKET_RT_SHADOWS=1` on a native adapter with ray queries. The renderer's device then enables
//! `EXPERIMENTAL_RAY_QUERY` (gpu.rs); this module keeps bottom-level acceleration structures for
//! the static meshes of the mesh pool, rebuilds the top level from the instances' drawn poses when
//! something changed or moves, and supplies a forward shader whose `shadow_factor` traces one
//! inline ray query toward the sun instead of sampling the cascaded shadow map, which is then not
//! rendered. WebGPU, the baseline draw path and the default native path never reach any of this.
//!
//! Casters are the instances the cascades would draw (alive, visible, casting shadows). An
//! alpha-masked material's instances use a non-opaque copy of their mesh's bottom level, and the
//! shadow ray tests each of their candidate hits against the material's alpha cutoff as the masked
//! shadow pass does (rt_shadows.wgsl). Skinned meshes, whose vertices the skinning pass writes each
//! frame, get their bottom levels rebuilt in the frame's encoder after that pass, with the top
//! level, every frame they cast.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Quat, Vec3, Vec4};

use crate::Gpu;
use crate::meshes::MeshPool;
use crate::scene::{FLAG_ALIVE, FLAG_SHADOW, FLAG_VISIBLE, Scene, VARIANT_SHIFT};

/// The lighting group's bindings (forward.wgsl's group 1, rt_shadows.wgsl): the top level, the
/// casters' table, and the mesh pool's indices and vertices for masked casters' texture lookups.
pub const BINDINGS: [u32; 4] = [11, 12, 13, 14];

/// Whether `POCKET_RT_SHADOWS=1` asks for ray-traced sun shadows (never in the browser).
pub fn requested() -> bool {
    !cfg!(target_arch = "wasm32") && std::env::var("POCKET_RT_SHADOWS").is_ok_and(|v| v == "1")
}

/// What the last frame's top level held (FrameStats-style numbers for benchmarks).
#[derive(Clone, Copy, Debug, Default)]
pub struct RtShadowStats {
    /// Instances in the top-level structure.
    pub instances: u32,
    /// Of them, alpha-masked (tested per candidate hit).
    pub masked: u32,
    /// Bottom-level structures built so far (static meshes, plus masked copies).
    pub meshes: u32,
    /// Whether the top level was rebuilt this frame (something moved or appeared).
    pub rebuilt: bool,
}

/// One top-level instance's material and mesh range (`RtCaster` in rt_shadows.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CasterGpu {
    material: u32,
    first_index: u32,
    base_vertex: i32,
    _pad: u32,
}

pub struct RtShadows {
    /// Opaque bottom levels by mesh id (skinned meshes' too, rebuilt every frame they cast).
    /// Per mesh row; none for the rows of coarser levels of detail (shadow rays always meet an
    /// instance's full mesh).
    blas: Vec<Option<wgpu::Blas>>,
    /// Skinned meshes' geometry, by mesh id: what their per-frame rebuilds read.
    skinned: HashMap<u32, wgpu::BlasTriangleGeometrySizeDescriptor>,
    /// The last build held skinned casters, so the next frame rebuilds as well.
    skinned_casters: bool,
    /// Non-opaque copies for meshes drawn with alpha-masked materials, built on first use.
    masked: HashMap<u32, wgpu::Blas>,
    wanted_masked: Vec<u32>,
    tlas: wgpu::Tlas,
    casters: wgpu::Buffer,
    capacity: u32,
    /// Bumped when the top level or its table is replaced (bind groups hold them).
    pub generation: u64,
    /// Instances in the last build (entries past it are cleared when fewer follow).
    built: usize,
    /// Slots changed since the last build (the renderer reports its dirty uploads).
    stale: bool,
    pub stats: RtShadowStats,
}

fn tlas(device: &wgpu::Device, capacity: u32) -> wgpu::Tlas {
    device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: Some("ray-traced shadow casters"),
        max_instances: capacity,
        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
    })
}

fn casters(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ray-traced shadow caster table"),
        size: u64::from(capacity) * std::mem::size_of::<CasterGpu>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// A drawn pose as a TLAS row-major 3x4 transform: the cull pass's interpolation (common.wgsl
/// `instance_pose`: lerp the position, nlerp the rotation), then rotation times scale.
fn transform(slot: &crate::scene::InstanceGpu, alpha: f32) -> [f32; 12] {
    let pos = Vec3::from(slot.prev_pos).lerp(Vec3::from(slot.pos), alpha);
    let a = Vec4::from(slot.prev_rot);
    let b = Vec4::from(slot.rot);
    let b = if a.dot(b) < 0.0 { -b } else { b };
    let rot = Quat::from_vec4(a.lerp(b, alpha).normalize());
    let m = Mat3::from_quat(rot) * Mat3::from_diagonal(Vec3::from(slot.scale));
    let (r0, r1, r2) = (m.row(0), m.row(1), m.row(2));
    [
        r0.x, r0.y, r0.z, pos.x, r1.x, r1.y, r1.z, pos.y, r2.x, r2.y, r2.z, pos.z,
    ]
}

/// Mesh `id`'s triangles as a bottom level describes them.
fn geometry_size(
    meshes: &MeshPool,
    id: u32,
    opaque: bool,
) -> wgpu::BlasTriangleGeometrySizeDescriptor {
    wgpu::BlasTriangleGeometrySizeDescriptor {
        vertex_format: wgpu::VertexFormat::Float32x3,
        vertex_count: meshes.vertex_counts[id as usize],
        index_format: Some(wgpu::IndexFormat::Uint32),
        index_count: Some(meshes.infos[id as usize].index_count),
        flags: if opaque {
            wgpu::AccelerationStructureGeometryFlags::OPAQUE
        } else {
            wgpu::AccelerationStructureGeometryFlags::empty()
        },
    }
}

fn create_blas(
    device: &wgpu::Device,
    size: &wgpu::BlasTriangleGeometrySizeDescriptor,
    fast_build: bool,
) -> wgpu::Blas {
    device.create_blas(
        &wgpu::CreateBlasDescriptor {
            label: Some("shadow caster mesh"),
            flags: if fast_build {
                wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD
            } else {
                wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE
            },
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        },
        wgpu::BlasGeometrySizeDescriptors::Triangles {
            descriptors: vec![size.clone()],
        },
    )
}

/// The build of `blas` from mesh `id` of the pool.
fn build_entry<'a>(
    blas: &'a wgpu::Blas,
    size: &'a wgpu::BlasTriangleGeometrySizeDescriptor,
    meshes: &'a MeshPool,
    id: u32,
) -> wgpu::BlasBuildEntry<'a> {
    let info = meshes.infos[id as usize];
    wgpu::BlasBuildEntry {
        blas,
        geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
            size,
            vertex_buffer: &meshes.vertices,
            first_vertex: info.base_vertex as u32,
            vertex_stride: std::mem::size_of::<pocket_assets::Vertex>() as u64,
            index_buffer: Some(&meshes.indices),
            first_index: Some(info.first_index),
            transform_buffer: None,
            transform_buffer_offset: None,
        }]),
    }
}

/// Bottom levels for the static meshes `ids` of the pool, opaque or not, built in one submission.
fn build_blas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    meshes: &MeshPool,
    ids: &[u32],
    opaque: bool,
) -> Vec<wgpu::Blas> {
    let sizes: Vec<_> = ids
        .iter()
        .map(|&id| geometry_size(meshes, id, opaque))
        .collect();
    let blas: Vec<_> = sizes
        .iter()
        .map(|size| create_blas(device, size, false))
        .collect();
    let entries: Vec<_> = blas
        .iter()
        .zip(&sizes)
        .zip(ids)
        .map(|((blas, size), &id)| build_entry(blas, size, meshes, id))
        .collect();
    if !entries.is_empty() {
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("shadow caster meshes"),
        });
        enc.build_acceleration_structures(&entries, std::iter::empty());
        queue.submit([enc.finish()]);
    }
    drop(entries);
    blas
}

impl RtShadows {
    /// Present when the renderer's device has ray queries (`Capabilities::ray_query`, which gpu.rs
    /// sets only when [`requested`]).
    pub fn new(gpu: &Gpu) -> Option<RtShadows> {
        if !gpu.caps.ray_query {
            return None;
        }
        log::info!("ray-traced sun shadows: on (POCKET_RT_SHADOWS=1)");
        Some(RtShadows {
            blas: Vec::new(),
            skinned: HashMap::new(),
            skinned_casters: false,
            masked: HashMap::new(),
            wanted_masked: Vec::new(),
            tlas: tlas(&gpu.device, 64),
            casters: casters(&gpu.device, 64),
            capacity: 64,
            generation: 0,
            built: 0,
            stale: true,
            stats: RtShadowStats::default(),
        })
    }

    /// The lighting layout's entries for the top level, the casters' table and the mesh pool.
    pub fn layout_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        vec![
            wgpu::BindGroupLayoutEntry {
                binding: BINDINGS[0],
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::AccelerationStructure {
                    vertex_return: false,
                },
                count: None,
            },
            storage(BINDINGS[1]),
            storage(BINDINGS[2]),
            storage(BINDINGS[3]),
        ]
    }

    pub fn bind_entries<'a>(&'a self, meshes: &'a MeshPool) -> Vec<wgpu::BindGroupEntry<'a>> {
        vec![
            wgpu::BindGroupEntry {
                binding: BINDINGS[0],
                resource: self.tlas.as_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BINDINGS[1],
                resource: self.casters.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BINDINGS[2],
                resource: meshes.indices.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BINDINGS[3],
                resource: meshes.vertices.as_entire_binding(),
            },
        ]
    }

    /// The forward shader with `shadow_factor` replaced by rt_shadows.wgsl's ray query (the
    /// cascaded version stays, unused, as `csm_shadow_factor`).
    pub fn forward_source() -> String {
        let forward = crate::shaders::source("forward");
        assert_eq!(
            forward.matches("fn shadow_factor(").count(),
            1,
            "forward.wgsl defines shadow_factor once"
        );
        format!(
            "enable wgpu_ray_query;\n{}\n{}",
            forward.replacen("fn shadow_factor(", "fn csm_shadow_factor(", 1),
            include_str!("../shaders/rt_shadows.wgsl")
        )
    }

    /// The forward module for ray-traced shadows, with all of naga's runtime checks: without the
    /// ray-query tracker the one shadow ray per pixel was only about 1% cheaper
    /// (docs/spec/rt-shadows.md), unlike the research tracers' loops of queries.
    pub fn forward_module(device: &wgpu::Device) -> wgpu::ShaderModule {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("forward (ray-traced sun shadows)"),
            source: wgpu::ShaderSource::Wgsl(Self::forward_source().into()),
        })
    }

    /// Builds the bottom levels of static meshes added since the last call and the masked copies
    /// the last frame asked for, and grows the top level and its table to `slots` instances.
    /// Returns whether those were replaced (bind groups hold them).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        meshes: &MeshPool,
        slots: usize,
    ) -> bool {
        if self.blas.len() < meshes.len() {
            let first = self.blas.len() as u32;
            let ids: Vec<u32> = (first..meshes.len() as u32)
                .filter(|&id| !meshes.dynamic[id as usize] && !meshes.is_level(id))
                .collect();
            let mut built = build_blas(device, queue, meshes, &ids, true).into_iter();
            for id in first..meshes.len() as u32 {
                if meshes.is_level(id) {
                    self.blas.push(None);
                    continue;
                }
                let blas = if meshes.dynamic[id as usize] {
                    // Built in the frame's encoder whenever an instance casts (encode).
                    let size = geometry_size(meshes, id, true);
                    let blas = create_blas(device, &size, true);
                    self.skinned.insert(id, size);
                    blas
                } else {
                    built.next().unwrap_or_else(|| unreachable!())
                };
                self.blas.push(Some(blas));
            }
            // Instances of the new meshes join the next build.
            self.stale = true;
        }
        if !self.wanted_masked.is_empty() {
            let mut ids = std::mem::take(&mut self.wanted_masked);
            ids.sort_unstable();
            ids.dedup();
            ids.retain(|id| !self.masked.contains_key(id));
            for (id, blas) in ids
                .iter()
                .zip(build_blas(device, queue, meshes, &ids, false))
            {
                self.masked.insert(*id, blas);
            }
            self.stale = true;
        }
        self.stats.meshes = (self.blas.iter().flatten().count() + self.masked.len()) as u32;
        let need = (slots as u32).max(1);
        if need > self.capacity {
            self.capacity = need.next_power_of_two();
            self.tlas = tlas(device, self.capacity);
            self.casters = casters(device, self.capacity);
            self.built = 0;
            self.stale = true;
            self.generation += 1;
            return true;
        }
        false
    }

    /// The renderer uploaded changed slots (`changed`): the next frame rebuilds the top level.
    pub fn slots_changed(&mut self, changed: bool) {
        self.stale |= changed;
    }

    /// Writes this frame's casters at their drawn poses (`alpha` between the last two ticks) and
    /// their table, and encodes the top-level build, with the bottom levels of skinned casters
    /// (whose vertices the skinning pass wrote earlier in `enc`). A static scene keeps its last
    /// build: only changed slots, newly built meshes, interpolating poses or skinned casters
    /// rebuild it. A masked caster whose non-opaque copy is not built yet asks for it and joins one
    /// frame later.
    pub fn encode(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        meshes: &MeshPool,
        scene: &Scene,
        alpha: f32,
    ) {
        self.stats.rebuilt = self.stale || self.skinned_casters || scene.interpolating();
        if !self.stats.rebuilt {
            return;
        }
        self.stale = false;
        let need = FLAG_ALIVE | FLAG_VISIBLE | FLAG_SHADOW;
        let mut table = Vec::new();
        let mut masked = 0;
        let mut skinned = Vec::new();
        for slot in &scene.slots {
            if slot.flags & need != need || table.len() == self.capacity as usize {
                continue;
            }
            let Some(opaque) = self.blas.get(slot.mesh as usize).and_then(Option::as_ref) else {
                continue;
            };
            let is_skinned = self.skinned.contains_key(&slot.mesh);
            if is_skinned {
                skinned.push(slot.mesh);
            }
            // A skinned mesh's masked material casts opaque: its copy would need rebuilding too.
            let blas = if (slot.flags >> VARIANT_SHIFT) & 1 == 1 && !is_skinned {
                let Some(blas) = self.masked.get(&slot.mesh) else {
                    self.wanted_masked.push(slot.mesh);
                    self.stale = true;
                    continue;
                };
                masked += 1;
                blas
            } else {
                opaque
            };
            let info = meshes.infos[slot.mesh as usize];
            self.tlas[table.len()] = Some(wgpu::TlasInstance::new(
                blas,
                transform(slot, alpha),
                0,
                0xff,
            ));
            table.push(CasterGpu {
                material: slot.material,
                first_index: info.first_index,
                base_vertex: info.base_vertex,
                _pad: 0,
            });
        }
        for i in table.len()..self.built {
            self.tlas[i] = None;
        }
        if !table.is_empty() {
            queue.write_buffer(&self.casters, 0, bytemuck::cast_slice(&table));
        }
        self.stats.instances = table.len() as u32;
        self.stats.masked = masked;
        skinned.sort_unstable();
        skinned.dedup();
        self.skinned_casters = !skinned.is_empty();
        let entries: Vec<_> = skinned
            .iter()
            .filter_map(|&id| {
                let blas = self.blas[id as usize].as_ref()?;
                Some(build_entry(blas, &self.skinned[&id], meshes, id))
            })
            .collect();
        enc.build_acceleration_structures(&entries, [&self.tlas]);
        self.built = table.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CPU pose matches the cull pass's interpolation and wgpu's row-major 3x4 layout.
    #[test]
    fn transforms_interpolate_like_the_cull_pass() {
        let q = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let slot = crate::scene::InstanceGpu {
            prev_pos: [0.0, 0.0, 0.0],
            pos: [2.0, 4.0, 6.0],
            prev_rot: Quat::IDENTITY.to_array(),
            // The same rotation through the other hemisphere: nlerp takes the short way.
            rot: (-q).to_array(),
            scale: [1.0, 2.0, 3.0],
            ..Default::default()
        };
        let m = transform(&slot, 1.0);
        let expect = Mat3::from_quat(q) * Mat3::from_diagonal(Vec3::new(1.0, 2.0, 3.0));
        let world = |m: &[f32; 12], v: Vec3| {
            Vec3::new(
                m[0] * v.x + m[1] * v.y + m[2] * v.z + m[3],
                m[4] * v.x + m[5] * v.y + m[6] * v.z + m[7],
                m[8] * v.x + m[9] * v.y + m[10] * v.z + m[11],
            )
        };
        let v = Vec3::new(1.0, 1.0, 1.0);
        assert!((world(&m, v) - (expect * v + Vec3::new(2.0, 4.0, 6.0))).length() < 1e-5);
        let half = transform(&slot, 0.5);
        let rot = Quat::IDENTITY.lerp(q, 0.5).normalize();
        let expect = rot * (v * Vec3::new(1.0, 2.0, 3.0)) + Vec3::new(1.0, 2.0, 3.0);
        assert!((world(&half, v) - expect).length() < 1e-5);
    }

    /// The ray-traced forward variant still defines everything the forward pipelines need.
    #[test]
    fn forward_variant_replaces_only_the_shadow_lookup() {
        let source = RtShadows::forward_source();
        assert!(source.starts_with("enable wgpu_ray_query;"));
        assert_eq!(source.matches("fn shadow_factor(").count(), 1);
        assert_eq!(source.matches("fn csm_shadow_factor(").count(), 1);
        let module = wgpu::naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{e:?}"));
    }
}
