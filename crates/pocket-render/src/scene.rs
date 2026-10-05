//! The renderer's retained scene: one GPU instance slot per drawn part of an entity, kept in step
//! with the render feed. Only what changed is written to the GPU; a static world costs nothing per
//! frame on the CPU. Each slot keeps the last two ticks' poses so drawing interpolates between
//! them at the display's rate.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec3};
use pocket_assets::frame::{
    CameraView, EnvironmentView, InstanceUpdate, LightView, Look, Pose, RenderFrame, SeaView,
    SplatView,
};

pub const FLAG_ALIVE: u32 = 1;
pub const FLAG_VISIBLE: u32 = 2;
pub const FLAG_SHADOW: u32 = 4;
/// Bits 3-4 of the flags: the pipeline variant (1 alpha-masked, 2 double-sided).
pub const VARIANT_SHIFT: u32 = 3;
pub const VARIANTS: u32 = 4;

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
    pub local: Option<(Vec3, Quat, Vec3)>,
}

/// Turns an entity's look into parts; `None` while its assets are loading.
pub trait Resolve {
    fn parts(&mut self, entity: u64, look: &Look) -> Option<Vec<Part>>;
    /// The entity's parts are gone (its look changed or it was removed).
    fn released(&mut self, _entity: u64) {}
}

struct Entry {
    pose: Pose,
    look: Option<Look>,
    slots: Vec<u32>,
    /// Each slot's placement in the entity (asset nodes).
    locals: Vec<Option<(Vec3, Quat, Vec3)>>,
    /// Waiting for an asset.
    pending: bool,
}

#[derive(Default)]
pub struct Scene {
    pub slots: Vec<InstanceGpu>,
    slot_entity: Vec<u64>,
    free: Vec<u32>,
    entities: HashMap<u64, Entry>,
    dirty: Vec<u32>,
    dirty_mark: Vec<bool>,
    /// Slots whose previous pose differs from the current one (moved at the last tick).
    moving: Vec<u32>,
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
}

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

    pub fn instance_count(&self) -> usize {
        self.slots.len()
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
        self.entities.get(&entity).map_or(&[], |e| e.slots.as_slice())
    }

    /// The entity drawn in `slot`.
    pub fn entity_of_slot(&self, slot: u32) -> Option<u64> {
        self.slot_entity
            .get(slot as usize)
            .copied()
            .filter(|&e| e != 0)
    }

    fn mark(&mut self, slot: u32) {
        let i = slot as usize;
        if !self.dirty_mark[i] {
            self.dirty_mark[i] = true;
            self.dirty.push(slot);
        }
    }

    fn alloc(&mut self, entity: u64) -> u32 {
        if let Some(s) = self.free.pop() {
            self.slot_entity[s as usize] = entity;
            return s;
        }
        self.slots.push(InstanceGpu::default());
        self.slot_entity.push(entity);
        self.dirty_mark.push(false);
        (self.slots.len() - 1) as u32
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

    fn release(&mut self, entry_slots: Vec<u32>) {
        for s in entry_slots {
            let slot = self.slots[s as usize];
            if slot.flags & FLAG_ALIVE != 0 {
                let variant = (slot.flags >> VARIANT_SHIFT) & 3;
                self.count(slot.mesh * VARIANTS + variant, -1);
            }
            self.slots[s as usize] = InstanceGpu::default();
            self.slot_entity[s as usize] = 0;
            self.mark(s);
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
        // Slots that moved at the previous tick and not at this one stop interpolating.
        let mut still_moving: Vec<u32> = Vec::new();
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
                self.release(e.slots);
                self.anims.remove(&id);
                resolve.released(id);
            }
        }
        for u in frame.instances {
            self.update(u, resolve, &mut still_moving);
        }
        if new_tick {
            for s in moved_before {
                if !still_moving.contains(&s) && (s as usize) < self.slots.len() {
                    let i = &mut self.slots[s as usize];
                    i.prev_pos = i.pos;
                    i.prev_rot = i.rot;
                    self.mark(s);
                }
            }
            self.moving = still_moving;
        } else {
            self.moving.extend(still_moving);
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

    fn update(&mut self, u: InstanceUpdate, resolve: &mut dyn Resolve, moving: &mut Vec<u32>) {
        if let Some(a) = u.anim.clone() {
            self.anims.insert(u.id, a);
        }
        let exists = self.entities.contains_key(&u.id);
        if !exists {
            self.entities.insert(
                u.id,
                Entry {
                    pose: u.pose.unwrap_or_default(),
                    look: None,
                    slots: Vec::new(),
                    locals: Vec::new(),
                    pending: false,
                },
            );
        }
        let look_changed = u.look.is_some();
        if let Some(look) = u.look {
            let e = self
                .entities
                .get_mut(&u.id)
                .unwrap_or_else(|| unreachable!());
            e.look = Some(look);
        }
        if let Some(pose) = u.pose {
            let e = self
                .entities
                .get_mut(&u.id)
                .unwrap_or_else(|| unreachable!());
            e.pose = pose;
        }
        if look_changed || !exists {
            self.rebuild(u.id, resolve);
        } else if u.pose.is_some() {
            self.repose(u.id, moving);
        }
    }

    /// Re-places an entity's slots at its new pose, keeping the old one as the previous pose.
    fn repose(&mut self, id: u64, moving: &mut Vec<u32>) {
        let Some(e) = self.entities.get(&id) else {
            return;
        };
        if e.pending {
            return;
        }
        let pose = e.pose;
        let slots = e.slots.clone();
        let locals = e.locals.clone();
        for (k, s) in slots.iter().enumerate() {
            let local = locals.get(k).copied().flatten();
            let (p, r, sc) = compose(&pose, local);
            let i = &mut self.slots[*s as usize];
            i.prev_pos = i.pos;
            i.prev_rot = i.rot;
            i.pos = p;
            i.rot = r;
            i.scale = sc;
            moving.push(*s);
            self.mark(*s);
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
        let had_parts = !old.is_empty();
        self.release(old);
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
        for (k, part) in parts.iter().enumerate() {
            let s = self.alloc(id);
            let flags = flags | (part.variant << VARIANT_SHIFT);
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
            let _ = k;
            self.count(part.mesh * VARIANTS + part.variant, 1);
            self.mark(s);
            slots.push(s);
        }
        if let Some(e) = self.entities.get_mut(&id) {
            e.slots = slots;
            e.locals = parts.iter().map(|p| p.local).collect();
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
        let mut d = std::mem::take(&mut self.dirty);
        for &s in &d {
            if let Some(m) = self.dirty_mark.get_mut(s as usize) {
                *m = false;
            }
        }
        if self.full_upload {
            self.full_upload = false;
            return if self.slots.is_empty() {
                Vec::new()
            } else {
                vec![0..self.slots.len() as u32]
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

    /// Visible-list regions per batch (variant-major: `variant * meshes + mesh`, prefix sums of
    /// the instance counts) and the per-view stride.
    pub fn batch_offsets(&self, meshes: usize) -> (Vec<u32>, u32) {
        let mut offsets = Vec::with_capacity(meshes * VARIANTS as usize);
        let mut at = 0u32;
        for v in 0..VARIANTS as usize {
            for m in 0..meshes {
                offsets.push(at);
                at += self
                    .mesh_counts
                    .get(m * VARIANTS as usize + v)
                    .copied()
                    .unwrap_or(0);
            }
        }
        (offsets, at.max(1))
    }
}
