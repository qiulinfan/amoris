//! Ray-traced sun shadows (charter 4.4, Pioneer 2026-10-09; docs/spec/rt-shadows.md): opt-in with
//! `AMORIS_RT_SHADOWS=1` on a native adapter with ray queries. The renderer's device then enables
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
//! frame, get their bottom levels rebuilt after that pass, every frame they cast. On Metal, wgpu
//! 30 lacks BLAS-to-TLAS barriers (gfx-rs/wgpu#9215): bottom-level builds are completed before
//! building the top level. This introduces a CPU wait for each skinned frame until that upstream
//! fix is released and verified. The Metal ray-query forward shader also uses one sample: its
//! multisampled variant currently produces black frames on Apple M5.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Quat, Vec3, Vec4};

use crate::Gpu;
use crate::meshes::MeshPool;
use crate::scene::{FLAG_ALIVE, FLAG_SHADOW, FLAG_TRANSPARENT, FLAG_VISIBLE, Scene, VARIANT_SHIFT};

/// The lighting group's bindings (forward.wgsl's group 1, rt_shadows.wgsl): the top level, the
/// casters' table, and the mesh pool's indices and vertices for masked casters' texture lookups.
pub const BINDINGS: [u32; 4] = [11, 12, 13, 14];

/// Whether `AMORIS_RT_SHADOWS=1` asks for ray-traced sun shadows (never in the browser).
pub fn requested() -> bool {
    !cfg!(target_arch = "wasm32") && std::env::var("AMORIS_RT_SHADOWS").is_ok_and(|v| v == "1")
}

/// What a device's ray queries are for: the sun's shadows (`AMORIS_RT_SHADOWS=1`), the water's
/// reflections (`AMORIS_RT_REFLECTIONS=1`, water.md 67), or both. Never in the browser.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RtEffects {
    pub shadows: bool,
    pub reflections: bool,
}
impl RtEffects {
    pub fn from_env() -> RtEffects {
        RtEffects {
            shadows: requested(),
            reflections: !cfg!(target_arch = "wasm32")
                && std::env::var("AMORIS_RT_REFLECTIONS").is_ok_and(|v| v == "1"),
        }
    }
    pub fn any(self) -> bool {
        self.shadows || self.reflections
    }
}

/// Instance masks (rt_common.wgsl): what the shadow rays meet, and what the reflected rays do.
const CASTS: u8 = 1;
const SEEN: u8 = 2;

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
    device: wgpu::Device,
    /// wgpu 30's Metal backend does not insert acceleration-structure barriers (#9215).
    /// Complete bottom-level builds before the top-level build consumes them.
    metal: bool,
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
    /// The effects the device's ray queries serve (`Capabilities::rt`).
    pub effects: RtEffects,
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
            vertex_stride: std::mem::size_of::<amoris_assets::Vertex>() as u64,
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
    metal: bool,
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
        let submission = queue.submit([enc.finish()]);
        if metal {
            wait_for_build(device, submission);
        }
    }
    drop(entries);
    blas
}

fn wait_for_build(device: &wgpu::Device, submission: wgpu::SubmissionIndex) {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .expect("Metal shadow acceleration-structure build completes");
}

fn complete_build(device: &wgpu::Device, queue: &wgpu::Queue, enc: &mut wgpu::CommandEncoder) {
    let next = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frame after shadow acceleration-structure build"),
    });
    let built = std::mem::replace(enc, next);
    wait_for_build(device, queue.submit([built.finish()]));
}

impl RtShadows {
    /// Present when the renderer's device has ray queries (`Capabilities::ray_query`, which gpu.rs
    /// sets only when [`requested`]).
    pub fn new(gpu: &Gpu) -> Option<RtShadows> {
        if !gpu.caps.ray_query {
            return None;
        }
        let effects = gpu.caps.rt;
        log::info!(
            "ray tracing: sun shadows {}, water reflections {}",
            if effects.shadows { "on" } else { "off" },
            if effects.reflections { "on" } else { "off" }
        );
        Some(RtShadows {
            effects,
            device: gpu.device.clone(),
            metal: gpu.info.backend == wgpu::Backend::Metal,
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

    /// The effective AA mode for this shader. Preserve TAA while avoiding the broken Metal
    /// multisampled ray-query variant; the ordinary forward shader keeps the adapter's defaults.
    pub(crate) fn antialiasing(&self, requested: crate::Antialiasing) -> crate::Antialiasing {
        use crate::Antialiasing;
        let effective = match (self.metal, requested) {
            (true, Antialiasing::Msaa) => Antialiasing::Off,
            (true, Antialiasing::MsaaTaa) => Antialiasing::Taa,
            _ => requested,
        };
        if effective != requested {
            log::warn!(
                "Metal ray-traced sun shadows: {} produces black frames; using {} (one sample)",
                requested.name(),
                effective.name()
            );
        }
        effective
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
    pub fn forward_source(effects: RtEffects) -> String {
        format!(
            "enable wgpu_ray_query;\n{}",
            Self::traced(&crate::shaders::source("forward"), effects)
        )
    }

    /// A forward shader (forward.wgsl and what is composed around it) with what `effects` trace:
    /// its `shadow_factor` (the cascaded one kept as `csm_shadow_factor`), its `water_reflect`
    /// (the screen-space one kept as `ssr_water_reflect`, water.md 67), or both. The module must
    /// begin with `enable wgpu_ray_query;`.
    pub fn traced(forward: &str, effects: RtEffects) -> String {
        let mut source = format!("{forward}\n{}", include_str!("../shaders/rt_common.wgsl"));
        for (on, name, kept, file) in [
            (
                effects.shadows,
                "shadow_factor",
                "csm_shadow_factor",
                include_str!("../shaders/rt_shadows.wgsl"),
            ),
            (
                effects.reflections,
                "water_reflect",
                "ssr_water_reflect",
                include_str!("../shaders/rt_reflections.wgsl"),
            ),
        ] {
            if !on {
                continue;
            }
            let defined = format!("fn {name}(");
            assert_eq!(
                source.matches(&defined).count(),
                1,
                "forward.wgsl defines {name} once"
            );
            source = format!(
                "{}\n{file}",
                source.replacen(&defined, &format!("fn {kept}("), 1)
            );
        }
        source
    }

    /// The forward module for its effects, with all of naga's runtime checks: without the
    /// ray-query tracker the one shadow ray per pixel was only about 1% cheaper
    /// (docs/spec/rt-shadows.md), unlike the research tracers' loops of queries.
    pub fn forward_module(&self, device: &wgpu::Device) -> wgpu::ShaderModule {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("forward (ray-traced)"),
            source: wgpu::ShaderSource::Wgsl(Self::forward_source(self.effects).into()),
        })
    }

    /// Builds the bottom levels of static meshes added since the last call, rebuilds those of the
    /// rows `replaced` (`MeshPool::replace`: remeshed chunks), builds the masked copies the last
    /// frame asked for, and grows the top level and its table to `slots` instances. Returns whether
    /// those were replaced (bind groups hold them). An empty mesh gets no bottom level.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        meshes: &MeshPool,
        slots: usize,
        replaced: &[u32],
    ) -> bool {
        let empty = |id: u32| meshes.infos[id as usize].index_count == 0;
        let old = self.blas.len() as u32;
        let again: Vec<u32> = replaced
            .iter()
            .copied()
            .filter(|&id| id < old && !meshes.dynamic[id as usize] && !meshes.is_level(id))
            .collect();
        if !again.is_empty() {
            let build: Vec<u32> = again.iter().copied().filter(|&id| !empty(id)).collect();
            let mut built = build_blas(device, queue, meshes, &build, true, self.metal).into_iter();
            for &id in &again {
                self.masked.remove(&id);
                self.blas[id as usize] = if empty(id) { None } else { built.next() };
            }
            self.stale = true;
        }
        if self.blas.len() < meshes.len() {
            let first = self.blas.len() as u32;
            let ids: Vec<u32> = (first..meshes.len() as u32)
                .filter(|&id| !meshes.dynamic[id as usize] && !meshes.is_level(id) && !empty(id))
                .collect();
            let mut built = build_blas(device, queue, meshes, &ids, true, self.metal).into_iter();
            for id in first..meshes.len() as u32 {
                if meshes.is_level(id) {
                    self.blas.push(None);
                    continue;
                }
                if !meshes.dynamic[id as usize] && empty(id) {
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
                .zip(build_blas(device, queue, meshes, &ids, false, self.metal))
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
        let drawn = FLAG_ALIVE | FLAG_VISIBLE;
        let mut table = Vec::new();
        let mut masked = 0;
        let mut skinned = Vec::new();
        for slot in &scene.slots {
            if slot.flags & drawn != drawn || table.len() == self.capacity as usize {
                continue;
            }
            // The shadow rays meet the casters the cascades would draw; the reflected rays every
            // opaque instance drawn.
            let mut mask = 0;
            if self.effects.shadows && slot.flags & FLAG_SHADOW != 0 {
                mask |= CASTS;
            }
            if self.effects.reflections && slot.flags & FLAG_TRANSPARENT == 0 {
                mask |= SEEN;
            }
            if mask == 0 {
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
                mask,
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
        if self.metal && !entries.is_empty() {
            // The prefix also contains this frame's skinning writes. The top-level build must
            // see completed bottom levels; two builds in one Metal encoder have no barrier.
            enc.build_acceleration_structures(&entries, std::iter::empty());
            complete_build(&self.device, queue, enc);
            enc.build_acceleration_structures(std::iter::empty(), [&self.tlas]);
        } else {
            enc.build_acceleration_structures(&entries, [&self.tlas]);
        }
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

    /// Each ray-traced forward variant still defines everything the forward pipelines need and
    /// replaces only what its effects trace.
    #[test]
    fn forward_variants_replace_only_what_they_trace() {
        for (shadows, reflections) in [(true, false), (false, true), (true, true)] {
            let source = RtShadows::forward_source(RtEffects {
                shadows,
                reflections,
            });
            assert!(source.starts_with("enable wgpu_ray_query;"));
            assert_eq!(source.matches("fn shadow_factor(").count(), 1);
            assert_eq!(
                source.matches("fn csm_shadow_factor(").count(),
                usize::from(shadows)
            );
            assert_eq!(source.matches("fn water_reflect(").count(), 1);
            assert_eq!(
                source.matches("fn ssr_water_reflect(").count(),
                usize::from(reflections)
            );
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
}
