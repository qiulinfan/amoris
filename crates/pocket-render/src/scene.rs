//! The renderer's retained scene: one GPU instance slot per drawn part of an entity, kept in step
//! with the render feed. Only what changed is written to the GPU; a static world costs nothing per
//! frame on the CPU. Each slot keeps the last two ticks' poses so drawing interpolates between
//! them at the display's rate.

use std::collections::{BTreeSet, HashMap, hash_map};

use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec3};
use pocket_assets::frame::{
    CameraView, EnvironmentView, InstanceUpdate, LightView, Look, Pose, RenderFrame, SeaView,
    SplatView,
};

pub const FLAG_ALIVE: u32 = 1;
pub const FLAG_VISIBLE: u32 = 2;
pub const FLAG_SHADOW: u32 = 4;
/// Bits 3-5 of the flags: the pipeline variant (1 alpha-masked, 2 double-sided, 4 neural texture).
pub const VARIANT_SHIFT: u32 = 3;
pub const VARIANT_MASK: u32 = 7;
pub const VARIANTS: u32 = 8;
/// The variant bit of materials decoded from a neural texture (their own forward pipelines).
pub const NEURAL_VARIANT: u32 = 4;
/// Drawn by the transparent stage (transparent.rs), sorted on the CPU: the culling pass keeps the
/// slot out of the camera's list; the shadow cascades draw it as its variant says.
pub const FLAG_TRANSPARENT: u32 = 64;

/// One instance slot (matches `Instance` in common.wgsl, 80 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct InstanceGpu {
    pub pos: [f32; 3],
    pub mesh: u32,
    pub rot: [f32; 4],
    pub scale: [f32; 3],
    pub material: u32,
    pub prev_pos: [f32; 3],
    pub flags: u32,
    pub prev_rot: [f32; 4],
}

/// One drawn part of an entity: a mesh, its material row, and its placement in the entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    pub mesh: u32,
    pub material: u32,
    /// The pipeline variant its material needs.
    pub variant: u32,
    /// Its material blends or transmits: the transparent stage draws it.
    pub transparent: bool,
    /// Its material can cast a shadow (`MaterialPool::casts`: not a transparent one whose opacity
    /// never reaches its cutoff); else the slot casts none whatever its look says.
    pub casts: bool,
    pub local: Option<(Vec3, Quat, Vec3)>,
}

/// Turns an entity's look into parts; `None` while its assets are loading.
pub trait Resolve {
    fn parts(&mut self, entity: u64, look: &Look) -> Option<Vec<Part>>;
    /// The entity's parts are gone (its look changed or it was removed).
    fn released(&mut self, _entity: u64) {}
}

/// An entity as drawn. Kept small (72 bytes): moving an entity reads its entry and nothing else
/// of the entity, and a scene of a million entities holds a million entries
/// (docs/bench/dynamic-scenes.md).
struct Entry {
    pose: Pose,
    look: Option<Box<Look>>,
    slots: Slots,
    /// Waiting for an asset.
    pending: bool,
}

const _: () = assert!(std::mem::size_of::<Entry>() <= 72);

/// An entity's instance slots, one per part, with each part's placement in the entity (asset
/// nodes).
#[derive(Default)]
enum Slots {
    #[default]
    None,
    /// One part, placed at the entity's pose.
    One(u32),
    Placed(Box<Placed>),
}

struct Placed {
    slots: Vec<u32>,
    locals: Vec<Option<(Vec3, Quat, Vec3)>>,
}

impl Slots {
    fn new(slots: Vec<u32>, parts: &[Part]) -> Slots {
        match slots.len() {
            0 => Slots::None,
            1 if parts.len() == 1 && parts[0].local.is_none() => Slots::One(slots[0]),
            _ => Slots::Placed(Box::new(Placed {
                slots,
                locals: parts.iter().map(|p| p.local).collect(),
            })),
        }
    }

    fn as_slice(&self) -> &[u32] {
        match self {
            Slots::None => &[],
            Slots::One(s) => std::slice::from_ref(s),
            Slots::Placed(p) => &p.slots,
        }
    }

    /// The placement in the entity of its `k`-th part (`None`: at the entity's pose).
    fn local(&self, k: usize) -> Option<(Vec3, Quat, Vec3)> {
        match self {
            Slots::Placed(p) => p.locals.get(k).copied().flatten(),
            _ => None,
        }
    }
}

/// The slots to write to the GPU, each listed once.
#[derive(Default)]
struct Dirty {
    slots: Vec<u32>,
    /// A bit per slot: listed (a bit, so that the marks of a million slots stay in cache).
    marked: Vec<u64>,
}

impl Dirty {
    /// Makes room for the marks of `slots` slots.
    fn grow(&mut self, slots: usize) {
        self.marked.resize(slots.div_ceil(64), 0);
    }

    fn mark(&mut self, slot: u32) {
        let (w, bit) = ((slot >> 6) as usize, 1u64 << (slot & 63));
        let m = &mut self.marked[w];
        if *m & bit == 0 {
            *m |= bit;
            self.slots.push(slot);
        }
    }

    /// The listed slots, in the order they were marked; empties the list.
    fn take(&mut self) -> Vec<u32> {
        let d = std::mem::take(&mut self.slots);
        for &s in &d {
            self.marked[(s >> 6) as usize] &= !(1u64 << (s & 63));
        }
        d
    }
}

#[derive(Default)]
pub struct Scene {
    pub slots: Vec<InstanceGpu>,
    slot_entity: Vec<u64>,
    free: Vec<u32>,
    /// By entity id, hashed with foldhash: with the standard library's SipHash, hashing limits how
    /// many lookups the CPU overlaps, and `apply`'s lookups ahead gain nothing
    /// (docs/bench/dynamic-scenes.md).
    entities: HashMap<u64, Entry, foldhash::fast::RandomState>,
    dirty: Dirty,
    /// Slots whose previous pose differs from the current one (re-placed at the last tick), each
    /// listed once.
    moving: Vec<u32>,
    /// Per slot: the tick, counted by `tick_mark`, at which it was last re-placed. A slot is in
    /// this tick's `moving` list exactly when it holds the current mark, so keeping the list costs
    /// one step per re-placed slot (docs/bench/dynamic-scenes.md).
    moved_at: Vec<u32>,
    /// Counts the ticks applied, from 1 (a mark of 0: not re-placed since a reset).
    tick_mark: u32,
    /// The allocation of the list `moving` replaced, reused for the next tick's.
    spare: Vec<u32>,
    pub mesh_counts: Vec<u32>,
    pub counts_changed: bool,
    pub lights: Vec<LightView>,
    pub lights_changed: bool,
    pub cameras: Vec<CameraView>,
    pub environment: Option<EnvironmentView>,
    pub environment_changed: bool,
    pub sea: Option<SeaView>,
    pub splats: Vec<SplatView>,
    pub splats_changed: bool,
    /// The world's particle emitters.
    pub emitters: Vec<pocket_assets::frame::EmitterView>,
    /// The world's sound sources (for the audio presenter).
    pub audio: Vec<pocket_assets::frame::AudioView>,
    /// The game UI's elements.
    pub ui: Vec<pocket_assets::frame::UiView>,
    /// Animation state per entity (skeletal animation).
    pub anims: HashMap<u64, pocket_assets::frame::AnimView>,
    pub tick: u64,
    pub t_s: f64,
    pub dt_s: f64,
    /// Everything must be rewritten to the GPU (a reset, or a buffer that grew).
    pub full_upload: bool,
    pending: usize,
    /// Live, resolved mesh slots whose look is visible, before GPU frustum/occlusion culling.
    visible_instances: usize,
    /// The live slots drawn by the transparent stage (`FLAG_TRANSPARENT`), visible or not.
    transparent: BTreeSet<u32>,
}

/// Entity updates looked up ahead of applying them (`Scene::apply`): enough to overlap many cache
/// misses, few enough that their entries stay in cache (64 to 1,024 measured alike).
const UPDATE_CHUNK: usize = 256;

fn compose(pose: &Pose, local: Option<(Vec3, Quat, Vec3)>) -> ([f32; 3], [f32; 4], [f32; 3]) {
    let p = Vec3::from(pose.position);
    let r = Quat::from_array(pose.rotation);
    let s = Vec3::from(pose.scale);
    match local {
        None => (pose.position, pose.rotation, pose.scale),
        Some((lp, lr, ls)) => (
            (p + r * (s * lp)).to_array(),
            (r * lr).normalize().to_array(),
            (s * ls).to_array(),
        ),
    }
}

impl Scene {
    pub fn new() -> Scene {
        Scene {
            dt_s: 1.0 / 60.0,
            ..Scene::default()
        }
    }

    /// Whether some slot is drawn between two poses, so its drawn pose changes from frame to frame
    /// (ray-traced shadows rebuild their acceleration structure then; rt_shadows.rs).
    pub fn interpolating(&self) -> bool {
        !self.moving.is_empty()
    }

    /// The slots drawn between two different poses (TAA reads their last pose; renderer.rs).
    pub fn moving_slots(&self) -> &[u32] {
        &self.moving
    }

    /// Slot high-water mark, including holes: the GPU scans this whole range.
    pub fn instance_count(&self) -> usize {
        self.slots.len()
    }

    /// Whether the opaque passes have any resolved, visible mesh instances to consider.
    /// Unlike the slot high-water mark, this becomes false when all instances are removed,
    /// hidden or waiting for assets. Actual frustum/occlusion visibility remains GPU work.
    pub fn has_visible_instances(&self) -> bool {
        self.visible_instances != 0
    }

    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    pub fn pending(&self) -> usize {
        self.pending
    }

    /// An entity's latest position, if it is drawn.
    pub fn position_of(&self, entity: u64) -> Option<[f32; 3]> {
        self.entities.get(&entity).map(|e| e.pose.position)
    }

    /// The instance slots an entity is drawn with (one per part).
    pub fn slots_of(&self, entity: u64) -> &[u32] {
        self.entities
            .get(&entity)
            .map_or(&[], |e| e.slots.as_slice())
    }

    /// The live slots whose material blends or transmits (the transparent stage's; in slot order).
    pub fn transparent_slots(&self) -> &BTreeSet<u32> {
        &self.transparent
    }

    /// The entity drawn in `slot`.
    pub fn entity_of_slot(&self, slot: u32) -> Option<u64> {
        self.slot_entity
            .get(slot as usize)
            .copied()
            .filter(|&e| e != 0)
    }

    fn alloc(&mut self, entity: u64) -> u32 {
        if let Some(s) = self.free.pop() {
            self.slot_entity[s as usize] = entity;
            return s;
        }
        self.slots.push(InstanceGpu::default());
        self.slot_entity.push(entity);
        self.dirty.grow(self.slots.len());
        self.moved_at.push(0);
        (self.slots.len() - 1) as u32
    }

    /// Starts the next tick's mark (see `moved_at`).
    fn next_tick_mark(&mut self) {
        self.tick_mark = self.tick_mark.wrapping_add(1);
        if self.tick_mark == 0 {
            // After 2^32 ticks the marks start again: no slot may hold the new one.
            self.moved_at.fill(0);
            self.tick_mark = 1;
        }
    }

    /// Counts per (mesh, variant), at `mesh * VARIANTS + variant`.
    fn count(&mut self, key: u32, delta: i64) {
        let m = key as usize;
        if self.mesh_counts.len() <= m {
            self.mesh_counts.resize(m + 1, 0);
        }
        self.mesh_counts[m] = (i64::from(self.mesh_counts[m]) + delta).max(0) as u32;
        self.counts_changed = true;
    }

    fn release(&mut self, entry_slots: &[u32]) {
        for &s in entry_slots {
            let slot = self.slots[s as usize];
            if slot.flags & FLAG_ALIVE != 0 {
                if slot.flags & FLAG_VISIBLE != 0 {
                    self.visible_instances -= 1;
                }
                let variant = (slot.flags >> VARIANT_SHIFT) & VARIANT_MASK;
                self.count(slot.mesh * VARIANTS + variant, -1);
                if slot.flags & FLAG_TRANSPARENT != 0 {
                    self.transparent.remove(&s);
                }
            }
            self.slots[s as usize] = InstanceGpu::default();
            self.slot_entity[s as usize] = 0;
            self.dirty.mark(s);
            self.free.push(s);
        }
    }

    fn clear(&mut self) {
        *self = Scene {
            dt_s: self.dt_s,
            mesh_counts: vec![0; self.mesh_counts.len()],
            counts_changed: true,
            full_upload: true,
            lights_changed: true,
            environment_changed: true,
            splats_changed: true,
            ..Scene::default()
        };
    }

    /// Applies one frame of the feed.
    pub fn apply(&mut self, frame: RenderFrame, resolve: &mut dyn Resolve) {
        if frame.reset {
            self.clear();
        }
        let new_tick = frame.tick != self.tick;
        self.tick = frame.tick;
        self.t_s = frame.t_s;
        if frame.dt_s > 0.0 {
            self.dt_s = frame.dt_s;
        }
        if new_tick || self.tick_mark == 0 {
            self.next_tick_mark();
        }
        // Slots that moved at the previous tick and not at this one stop interpolating.
        let mut still_moving = std::mem::take(&mut self.spare);
        still_moving.clear();
        let moved_before = if new_tick {
            std::mem::take(&mut self.moving)
        } else {
            Vec::new()
        };
        for id in frame.removed {
            if let Some(e) = self.entities.remove(&id) {
                if e.pending {
                    self.pending -= 1;
                }
                self.release(e.slots.as_slice());
                self.anims.remove(&id);
                resolve.released(id);
            }
        }
        // In chunks: a first pass looks a chunk's entities up, loads the CPU overlaps, so the second,
        // which applies the updates, finds their entries in cache. With a million entities a lookup
        // misses the cache, and applying one update at a time waited for each miss in turn
        // (docs/bench/dynamic-scenes.md).
        let mut updates = frame.instances.into_iter();
        loop {
            let ahead = updates.as_slice();
            if ahead.is_empty() {
                break;
            }
            for u in &ahead[..ahead.len().min(UPDATE_CHUNK)] {
                if let Some(e) = self.entities.get(&u.id) {
                    std::hint::black_box(e.pending);
                }
            }
            for u in updates.by_ref().take(UPDATE_CHUNK) {
                self.update(u, resolve, &mut still_moving);
            }
        }
        if new_tick {
            for &s in &moved_before {
                let i = s as usize;
                // A slot re-placed at this tick holds this tick's mark.
                if i < self.slots.len() && self.moved_at[i] != self.tick_mark {
                    let slot = &mut self.slots[i];
                    slot.prev_pos = slot.pos;
                    slot.prev_rot = slot.rot;
                    self.dirty.mark(s);
                }
            }
            self.moving = still_moving;
            self.spare = moved_before;
        } else {
            // Holds only slots not listed yet at this tick.
            self.moving.extend_from_slice(&still_moving);
            self.spare = still_moving;
        }
        if let Some(l) = frame.lights {
            self.lights = l;
            self.lights_changed = true;
        }
        if let Some(c) = frame.cameras {
            self.cameras = c;
        }
        if let Some(e) = frame.environment {
            self.environment = Some(e);
            self.environment_changed = true;
        }
        if let Some(s) = frame.sea {
            self.sea = s;
        }
        if let Some(s) = frame.splats {
            self.splats = s;
            self.splats_changed = true;
        }
        if let Some(u) = frame.ui {
            self.ui = u;
        }
        if let Some(a) = frame.audio {
            self.audio = a;
        }
        if let Some(e) = frame.emitters {
            self.emitters = e;
        }
    }

    /// Applies one entity's update; adds the slots it re-places that are not listed at this tick
    /// yet to `moving`.
    fn update(&mut self, u: InstanceUpdate, resolve: &mut dyn Resolve, moving: &mut Vec<u32>) {
        if let Some(a) = u.anim {
            self.anims.insert(u.id, a);
        }
        let e = match self.entities.entry(u.id) {
            hash_map::Entry::Vacant(v) => {
                v.insert(Entry {
                    pose: u.pose.unwrap_or_default(),
                    look: u.look.map(Box::new),
                    slots: Slots::None,
                    pending: false,
                });
                self.rebuild(u.id, resolve);
                return;
            }
            hash_map::Entry::Occupied(o) => o.into_mut(),
        };
        if let Some(pose) = u.pose {
            e.pose = pose;
        }
        if let Some(look) = u.look {
            e.look = Some(Box::new(look));
            self.rebuild(u.id, resolve);
        } else if u.pose.is_some() && !e.pending {
            // Re-places the entity's slots at its new pose, keeping the old one as the previous
            // pose.
            for (k, &s) in e.slots.as_slice().iter().enumerate() {
                let (p, r, sc) = compose(&e.pose, e.slots.local(k));
                let i = &mut self.slots[s as usize];
                i.prev_pos = i.pos;
                i.prev_rot = i.rot;
                i.pos = p;
                i.rot = r;
                i.scale = sc;
                let at = &mut self.moved_at[s as usize];
                if *at != self.tick_mark {
                    *at = self.tick_mark;
                    moving.push(s);
                }
                self.dirty.mark(s);
            }
        }
    }

    /// Makes an entity's slots from its look (after a look change, or when its assets arrive).
    fn rebuild(&mut self, id: u64, resolve: &mut dyn Resolve) {
        let Some(e) = self.entities.get_mut(&id) else {
            return;
        };
        let old = std::mem::take(&mut e.slots);
        let was_pending = e.pending;
        let pose = e.pose;
        let look = e.look.clone();
        let had_parts = !old.as_slice().is_empty();
        self.release(old.as_slice());
        if had_parts {
            resolve.released(id);
        }
        let Some(look) = look else {
            return;
        };
        let parts = resolve.parts(id, &look);
        let e = self.entities.get_mut(&id).unwrap_or_else(|| unreachable!());
        let Some(parts) = parts else {
            if !was_pending {
                e.pending = true;
                self.pending += 1;
            }
            return;
        };
        if was_pending {
            e.pending = false;
            self.pending -= 1;
        }
        let mut flags = FLAG_ALIVE;
        if look.visible {
            flags |= FLAG_VISIBLE;
        }
        if look.cast_shadows {
            flags |= FLAG_SHADOW;
        }
        let mut slots = Vec::with_capacity(parts.len());
        for part in &parts {
            let s = self.alloc(id);
            let mut flags = flags | (part.variant << VARIANT_SHIFT);
            if part.transparent {
                flags |= FLAG_TRANSPARENT;
                self.transparent.insert(s);
            }
            if !part.casts {
                flags &= !FLAG_SHADOW;
            }
            let (p, r, sc) = compose(&pose, part.local);
            self.slots[s as usize] = InstanceGpu {
                pos: p,
                mesh: part.mesh,
                rot: r,
                scale: sc,
                material: part.material,
                prev_pos: p,
                flags,
                prev_rot: r,
            };
            self.visible_instances += usize::from(flags & FLAG_VISIBLE != 0);
            self.count(part.mesh * VARIANTS + part.variant, 1);
            self.dirty.mark(s);
            slots.push(s);
        }
        if let Some(e) = self.entities.get_mut(&id) {
            e.slots = Slots::new(slots, &parts);
        }
    }

    /// Retries entities whose assets were loading.
    pub fn retry_pending(&mut self, resolve: &mut dyn Resolve) {
        if self.pending == 0 {
            return;
        }
        let ids: Vec<u64> = self
            .entities
            .iter()
            .filter(|(_, e)| e.pending)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.rebuild(id, resolve);
        }
    }

    /// The slots to write to the GPU, as contiguous runs; clears the dirty set.
    pub fn take_dirty_runs(&mut self) -> Vec<std::ops::Range<u32>> {
        let mut d = self.dirty.take();
        if self.full_upload {
            self.full_upload = false;
            return if self.slots.is_empty() {
                Vec::new()
            } else {
                std::iter::once(0..self.slots.len() as u32).collect()
            };
        }
        d.sort_unstable();
        let mut runs: Vec<std::ops::Range<u32>> = Vec::new();
        for s in d {
            match runs.last_mut() {
                Some(r) if r.end == s => r.end = s + 1,
                _ => runs.push(s..s + 1),
            }
        }
        runs
    }

    /// The instances of variant `v` that mesh row `row` may draw: those of the mesh whose instances
    /// it draws, `owners[row]` (itself, or for a level row its mesh; docs/spec/lod.md), none for
    /// `u32::MAX`.
    fn row_count(&self, owners: &[u32], row: usize, v: usize) -> u32 {
        match owners.get(row) {
            Some(&o) if o != u32::MAX => self
                .mesh_counts
                .get(o as usize * VARIANTS as usize + v)
                .copied()
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// Instances per batch, in `batch_offsets`' order (variant-major: `variant * rows + row`), for
    /// mesh rows drawing the instances of `owners` (one entry per row).
    pub fn batch_sizes(&self, owners: &[u32]) -> Vec<u32> {
        (0..VARIANTS as usize)
            .flat_map(|v| (0..owners.len()).map(move |r| self.row_count(owners, r, v)))
            .collect()
    }

    /// Visible-list regions per batch (variant-major: `variant * rows + row`, prefix sums of the
    /// sizes [`Scene::batch_sizes`] gives) and the per-view stride.
    pub fn batch_offsets(&self, owners: &[u32]) -> (Vec<u32>, u32) {
        let mut offsets = Vec::with_capacity(owners.len() * VARIANTS as usize);
        let mut at = 0u32;
        for v in 0..VARIANTS as usize {
            for r in 0..owners.len() {
                offsets.push(at);
                at += self.row_count(owners, r, v);
            }
        }
        (offsets, at.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Assets {
        ready: bool,
    }

    impl Resolve for Assets {
        fn parts(&mut self, _: u64, look: &Look) -> Option<Vec<Part>> {
            if look.mesh == "pending" && !self.ready {
                return None;
            }
            Some(if look.mesh == "empty" {
                Vec::new()
            } else {
                // Every raster mesh variant can contribute to the opaque passes. Models may
                // expand one entity into several slots, each released or rebuilt together; a
                // "glass" model adds a transparent part that casts and one that cannot.
                let mut parts: Vec<Part> = (0..VARIANTS)
                    .map(|variant| Part {
                        mesh: 0,
                        material: variant,
                        variant,
                        transparent: false,
                        casts: true,
                        local: None,
                    })
                    .collect();
                if look.mesh == "glass" {
                    for casts in [true, false] {
                        parts.push(Part {
                            mesh: 1,
                            material: 9,
                            variant: 1,
                            transparent: true,
                            casts,
                            local: None,
                        });
                    }
                }
                parts
            })
        }
    }

    fn instance(id: u64, mesh: &str, visible: bool) -> InstanceUpdate {
        InstanceUpdate {
            id,
            pose: Some(Pose::default()),
            look: Some(Look {
                mesh: mesh.into(),
                material: String::new(),
                color: [1.0; 4],
                metallic: 0.0,
                roughness: 0.6,
                transmission: None,
                ior: None,
                emissive: [0.0; 3],
                cast_shadows: true,
                visible,
            }),
            anim: None,
        }
    }

    fn update(
        scene: &mut Scene,
        assets: &mut Assets,
        instances: Vec<InstanceUpdate>,
        removed: Vec<u64>,
    ) {
        scene.apply(
            RenderFrame {
                instances,
                removed,
                ..RenderFrame::default()
            },
            assets,
        );
    }

    #[test]
    fn transparent_slots_follow_rebuild_hide_remove_and_reset() {
        let mut scene = Scene::new();
        let mut assets = Assets { ready: true };
        let transparent = |scene: &Scene| {
            scene
                .transparent_slots()
                .iter()
                .copied()
                .collect::<Vec<_>>()
        };
        update(
            &mut scene,
            &mut assets,
            vec![instance(1, "glass", true), instance(2, "model", true)],
            vec![],
        );
        let glass: Vec<u32> = scene.slots_of(1)[VARIANTS as usize..].to_vec();
        let mut sorted = glass.clone();
        sorted.sort_unstable();
        assert_eq!(transparent(&scene), sorted);
        for &s in scene.slots_of(1) {
            let flags = scene.slots[s as usize].flags;
            let is_glass = glass.contains(&s);
            assert_eq!(flags & FLAG_TRANSPARENT != 0, is_glass);
            // The part that cannot cast loses the look's shadow flag; the others keep it.
            let casts = !is_glass || s == glass[0];
            assert_eq!(flags & FLAG_SHADOW != 0, casts, "slot {s}");
        }
        // Hidden, the parts are rebuilt: still transparent (the stage skips invisible slots).
        update(
            &mut scene,
            &mut assets,
            vec![instance(1, "glass", false)],
            vec![],
        );
        assert_eq!(transparent(&scene).len(), 2);
        // A look without the transparent parts, then removal and a reset leave none.
        update(
            &mut scene,
            &mut assets,
            vec![instance(1, "model", true)],
            vec![],
        );
        assert!(transparent(&scene).is_empty());
        update(
            &mut scene,
            &mut assets,
            vec![instance(3, "glass", true)],
            vec![],
        );
        assert_eq!(transparent(&scene).len(), 2);
        update(&mut scene, &mut assets, vec![], vec![3]);
        assert!(transparent(&scene).is_empty());
        update(
            &mut scene,
            &mut assets,
            vec![instance(4, "glass", true)],
            vec![],
        );
        scene.apply(
            RenderFrame {
                reset: true,
                instances: vec![instance(5, "model", true)],
                ..RenderFrame::default()
            },
            &mut assets,
        );
        assert!(transparent(&scene).is_empty());
    }

    #[test]
    fn visible_instances_follow_hide_remove_reuse_and_reset_without_shrinking_slots() {
        let mut scene = Scene::new();
        let mut assets = Assets { ready: true };
        assert!(!scene.has_visible_instances());
        update(
            &mut scene,
            &mut assets,
            vec![instance(1, "model", true), instance(2, "model", true)],
            vec![],
        );
        let capacity = scene.instance_count();
        assert_eq!(capacity, 2 * VARIANTS as usize);
        assert!(scene.has_visible_instances());

        update(
            &mut scene,
            &mut assets,
            vec![instance(1, "model", false)],
            vec![],
        );
        assert!(scene.has_visible_instances(), "entity 2 remains visible");
        update(
            &mut scene,
            &mut assets,
            vec![instance(2, "model", false)],
            vec![],
        );
        assert!(
            !scene.has_visible_instances(),
            "hidden slots do not enable prepass probes"
        );
        assert_eq!(scene.instance_count(), capacity);

        update(
            &mut scene,
            &mut assets,
            vec![instance(1, "model", true)],
            vec![],
        );
        assert!(
            scene.has_visible_instances(),
            "a hidden entity becomes visible again"
        );
        update(&mut scene, &mut assets, vec![], vec![1, 2]);
        assert!(
            !scene.has_visible_instances(),
            "the last live parts were removed"
        );
        assert_eq!(
            scene.instance_count(),
            capacity,
            "GPU slot scan length stays unchanged"
        );

        update(
            &mut scene,
            &mut assets,
            vec![instance(3, "model", true)],
            vec![],
        );
        assert!(
            scene.has_visible_instances(),
            "released slots can be reused"
        );
        assert_eq!(scene.instance_count(), capacity);
        scene.apply(
            RenderFrame {
                reset: true,
                ..RenderFrame::default()
            },
            &mut assets,
        );
        assert!(
            !scene.has_visible_instances(),
            "reset clears the live count too"
        );
        assert_eq!(scene.instance_count(), 0);
    }

    #[test]
    fn pending_or_empty_assets_do_not_enable_prepass_probes() {
        let mut scene = Scene::new();
        let mut assets = Assets { ready: false };
        update(
            &mut scene,
            &mut assets,
            vec![instance(1, "pending", true)],
            vec![],
        );
        scene.retry_pending(&mut assets);
        assert_eq!(scene.entity_count(), 1);
        assert_eq!(scene.pending(), 1);
        assert!(
            !scene.has_visible_instances(),
            "an entity waiting for geometry has no drawable slots"
        );
        assets.ready = true;
        scene.retry_pending(&mut assets);
        assert!(
            scene.has_visible_instances(),
            "arriving geometry is eligible"
        );
        assert_eq!(scene.pending(), 0);
        let capacity = scene.instance_count();

        assets.ready = false;
        update(
            &mut scene,
            &mut assets,
            vec![instance(1, "pending", true)],
            vec![],
        );
        assert!(
            !scene.has_visible_instances(),
            "replacing a model releases its old geometry while loading"
        );
        assert_eq!(scene.instance_count(), capacity);
        update(&mut scene, &mut assets, vec![], vec![1]);
        assert_eq!(scene.pending(), 0);
        assert!(!scene.has_visible_instances());
        update(
            &mut scene,
            &mut assets,
            vec![instance(2, "empty", true)],
            vec![],
        );
        assert!(
            !scene.has_visible_instances(),
            "a loaded model without parts is still empty"
        );
    }

    /// Entities of one, two or three parts (`look.mesh` "1", "2", "3"), the parts placed apart
    /// in the entity.
    struct Parts;

    fn locals(mesh: &str) -> Vec<Option<(Vec3, Quat, Vec3)>> {
        let n: usize = mesh.parse().unwrap_or(1);
        (0..n)
            .map(|k| {
                (k > 0).then(|| {
                    (
                        Vec3::new(k as f32, 0.5, -0.25 * k as f32),
                        Quat::from_rotation_z(0.3 * k as f32),
                        Vec3::splat(0.5 + k as f32),
                    )
                })
            })
            .collect()
    }

    impl Resolve for Parts {
        fn parts(&mut self, _: u64, look: &Look) -> Option<Vec<Part>> {
            Some(
                locals(&look.mesh)
                    .into_iter()
                    .enumerate()
                    .map(|(k, local)| Part {
                        mesh: k as u32,
                        material: 0,
                        variant: 0,
                        transparent: false,
                        casts: true,
                        local,
                    })
                    .collect(),
            )
        }
    }

    /// A xorshift generator (deterministic, no dependency).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }

        fn pose(&mut self) -> Pose {
            let mut f = || (self.below(2001) as f32 - 1000.0) / 100.0;
            Pose {
                position: [f(), f(), f()],
                rotation: Quat::from_euler(glam::EulerRot::XYZ, f(), f(), f()).to_array(),
                scale: [1.0 + f().abs() / 10.0; 3],
            }
        }
    }

    /// What an entity should be drawn with: its parts, and the poses its slots are drawn between.
    struct Expected {
        mesh: String,
        prev: Pose,
        cur: Pose,
        /// Re-placed at the current tick.
        moved: bool,
    }

    fn look(mesh: &str) -> Look {
        Look {
            mesh: mesh.into(),
            ..instance(0, mesh, true)
                .look
                .unwrap_or_else(|| unreachable!())
        }
    }

    /// Random frames (entities added, moved, given a new look, removed; ticks repeated; a reset)
    /// against what they mean: after every frame each entity's slots are drawn from its previous
    /// to its current pose, an entity re-placed at an earlier tick and not at this one is drawn
    /// still, every slot drawn between two poses is listed as moving (once), and the dirty runs
    /// carry every changed slot to a GPU copy.
    #[test]
    fn applied_frames_move_and_stop_slots_as_their_poses_say() {
        let mut scene = Scene::new();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut expected: HashMap<u64, Expected> = HashMap::new();
        let mut gpu: Vec<InstanceGpu> = Vec::new();
        let mut next_id = 1u64;
        let mut tick = 1u64;
        for frame_no in 0..400 {
            let reset = frame_no == 200;
            let new_tick = reset || frame_no == 0 || rng.below(4) != 0;
            if new_tick {
                tick += 1;
            }
            if reset {
                expected.clear();
            }
            if new_tick {
                for e in expected.values_mut() {
                    e.moved = false;
                }
            }
            let mut instances = Vec::new();
            let mut removed = Vec::new();
            let ids: Vec<u64> = {
                let mut v: Vec<u64> = expected.keys().copied().collect();
                v.sort_unstable();
                v
            };
            for id in ids {
                let e = expected.get_mut(&id).unwrap_or_else(|| unreachable!());
                match rng.below(10) {
                    0 => {
                        removed.push(id);
                        expected.remove(&id);
                    }
                    1 => {
                        // A new look (and maybe a new pose): rebuilt, drawn still.
                        let mesh = (1 + rng.below(3)).to_string();
                        let pose = (rng.below(2) == 0).then(|| rng.pose());
                        if let Some(p) = pose {
                            e.cur = p;
                        }
                        e.prev = e.cur;
                        e.moved = false;
                        e.mesh.clone_from(&mesh);
                        instances.push(InstanceUpdate {
                            id,
                            pose,
                            look: Some(look(&mesh)),
                            anim: None,
                        });
                    }
                    2..=6 => {
                        let pose = rng.pose();
                        e.prev = e.cur;
                        e.cur = pose;
                        e.moved = true;
                        instances.push(InstanceUpdate {
                            id,
                            pose: Some(pose),
                            look: None,
                            anim: None,
                        });
                    }
                    _ => {}
                }
            }
            for _ in 0..rng.below(4) {
                let mesh = (1 + rng.below(3)).to_string();
                let pose = rng.pose();
                instances.push(InstanceUpdate {
                    id: next_id,
                    pose: Some(pose),
                    look: Some(look(&mesh)),
                    anim: None,
                });
                expected.insert(
                    next_id,
                    Expected {
                        mesh,
                        prev: pose,
                        cur: pose,
                        moved: false,
                    },
                );
                next_id += 1;
            }
            // At a new tick the entities not re-placed at it are drawn still.
            if new_tick {
                for e in expected.values_mut() {
                    if !e.moved {
                        e.prev = e.cur;
                    }
                }
            }
            scene.apply(
                RenderFrame {
                    tick,
                    reset,
                    instances,
                    removed,
                    ..RenderFrame::default()
                },
                &mut Parts,
            );

            // The GPU copy.
            gpu.resize(scene.slots.len(), InstanceGpu::default());
            for run in scene.take_dirty_runs() {
                let r = run.start as usize..run.end as usize;
                gpu[r.clone()].copy_from_slice(&scene.slots[r]);
            }
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&gpu),
                bytemuck::cast_slice::<_, u8>(&scene.slots),
                "frame {frame_no}: the dirty runs carry every change"
            );

            let moving = scene.moving_slots();
            let mut listed = moving.to_vec();
            listed.sort_unstable();
            listed.dedup();
            assert_eq!(listed.len(), moving.len(), "frame {frame_no}: listed once");
            assert_eq!(
                listed,
                marked_this_tick(&scene),
                "frame {frame_no}: the moving list is the slots holding this tick's mark"
            );
            for (id, e) in &expected {
                let slots = scene.slots_of(*id);
                let locals = locals(&e.mesh);
                assert_eq!(slots.len(), locals.len(), "frame {frame_no}: entity {id}");
                for (&s, &local) in slots.iter().zip(&locals) {
                    let (p, r, sc) = compose(&e.cur, local);
                    let (pp, pr, _) = compose(&e.prev, local);
                    let g = &scene.slots[s as usize];
                    assert_eq!(
                        (g.pos, g.rot, g.scale, g.prev_pos, g.prev_rot),
                        (p, r, sc, pp, pr),
                        "frame {frame_no}: entity {id}, slot {s}"
                    );
                    if e.prev != e.cur {
                        assert!(
                            listed.binary_search(&s).is_ok(),
                            "frame {frame_no}: entity {id}'s slot {s} drawn between two poses"
                        );
                    }
                }
            }
            if expected.values().any(|e| e.prev != e.cur) {
                assert!(scene.interpolating(), "frame {frame_no}");
            }
        }
        assert!(next_id > 100, "the frames made many entities");
    }

    /// One tick's frame turning the first `n` entities a little further (pose only).
    fn turned(n: u64, tick: u64) -> RenderFrame {
        RenderFrame {
            tick,
            instances: (1..=n)
                .map(|id| InstanceUpdate {
                    id,
                    pose: Some(Pose {
                        position: [id as f32, 0.0, 0.0],
                        rotation: Quat::from_rotation_y(tick as f32 * 0.01).to_array(),
                        scale: [1.0; 3],
                    }),
                    look: None,
                    anim: None,
                })
                .collect(),
            ..RenderFrame::default()
        }
    }

    /// The slots holding the current tick's mark (`Scene::moved_at`), in order.
    fn marked_this_tick(scene: &Scene) -> Vec<u32> {
        (0..scene.moved_at.len() as u32)
            .filter(|&s| scene.moved_at[s as usize] == scene.tick_mark)
            .collect()
    }

    /// A scene of `n` one-part entities, made at tick 1.
    fn scene_of(n: u64) -> Scene {
        let mut scene = Scene::new();
        let made = RenderFrame {
            tick: 1,
            instances: (1..=n)
                .map(|id| InstanceUpdate {
                    id,
                    pose: Some(Pose {
                        position: [id as f32, 0.0, 0.0],
                        ..Pose::default()
                    }),
                    look: Some(look("1")),
                    anim: None,
                })
                .collect(),
            ..RenderFrame::default()
        };
        scene.apply(made, &mut Parts);
        scene
    }

    /// A tick moving every one of 200,000 instances lists each once and finds the slots to stop
    /// by their marks: the moving list is exactly the slots holding the tick's mark, so stopping
    /// a slot is one load, not a search of the list (what made a tick quadratic before
    /// docs/bench/dynamic-scenes.md). A tick moving nothing stops them all.
    #[test]
    fn a_tick_moving_every_instance_lists_each_once_by_its_mark() {
        let n = 200_000u64;
        let mut scene = scene_of(n);
        for tick in 2..6 {
            scene.apply(turned(n, tick), &mut Parts);
            assert_eq!(scene.moving_slots().len(), n as usize, "tick {tick}");
            let mut listed = scene.moving_slots().to_vec();
            listed.sort_unstable();
            assert_eq!(listed, marked_this_tick(&scene), "tick {tick}");
        }
        scene.apply(turned(0, 6), &mut Parts);
        assert!(!scene.interpolating());
        assert!(marked_this_tick(&scene).is_empty());
        assert_eq!(scene.take_dirty_runs(), vec![0..n as u32]);
        assert!(
            scene
                .slots
                .iter()
                .all(|s| s.prev_pos == s.pos && s.prev_rot == s.rot),
            "every slot drawn still"
        );
    }

    /// The cost the test above pins by structure, timed (ignored: a timing is recorded, never a
    /// pass condition; run with `cargo test -p pocket-render --release --lib scene:: --
    /// --ignored --nocapture`). A tick moving or stopping every one of 200,000 instances costs
    /// less than twice making them; with this tick's list searched for each slot moved at the
    /// last one, as before docs/bench/dynamic-scenes.md, a tick took 121 s here in a debug build
    /// against 0.68 s for making them.
    #[test]
    #[ignore]
    fn a_tick_moving_every_instance_costs_about_what_making_them_does() {
        let n = 200_000u64;
        let t = std::time::Instant::now();
        let mut scene = scene_of(n);
        let make = t.elapsed();
        // The first moving tick has nothing to stop; the next ones check every slot moved before.
        scene.apply(turned(n, 2), &mut Parts);
        let mut best = std::time::Duration::MAX;
        for tick in 3..6 {
            let f = turned(n, tick);
            let t = std::time::Instant::now();
            scene.apply(f, &mut Parts);
            best = best.min(t.elapsed());
        }
        let t = std::time::Instant::now();
        scene.apply(turned(0, 6), &mut Parts);
        let stop = t.elapsed();
        eprintln!("made {n} in {make:?}; a moving tick {best:?}, a stopping tick {stop:?}");
        assert!(
            best < make * 2 && stop < make * 2,
            "a tick moving {n} instances took {best:?}, one stopping them {stop:?}: making them \
             took {make:?}"
        );
    }
}
