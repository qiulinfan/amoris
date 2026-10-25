//! The render feed (charter 4.4; host-protocol.md 5): what changed in the visual state of the
//! world after a tick, for the renderers. The game thread extracts a [`RenderFrame`] from the
//! components that changed (change detection, presentation only, never read by the simulation)
//! and pushes it to every subscriber's [`Mailbox`]; a renderer takes the merged pending frame when
//! it draws. A static world costs nothing per tick, and a renderer that falls behind receives one
//! merged frame, never a backlog.
//!
//! Values are `f32`: the feed is presentation, and positions are relative to the world origin.

// Presentation: merging frames for a subscriber; nothing here is read back by the simulation.
#![allow(clippy::disallowed_types)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[path = "checkpoint_feed.rs"]
mod checkpoint_feed;

/// An entity's pose for drawing: position, unit quaternion `[x, y, z, w]`, scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for Pose {
    fn default() -> Self {
        Pose {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

/// How an instance is drawn: the `Model` component in `f32`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Look {
    pub mesh: String,
    pub material: String,
    pub color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub transmission: Option<f32>,
    pub ior: Option<f32>,
    pub emissive: [f32; 3],
    pub cast_shadows: bool,
    pub visible: bool,
}

/// A skeletal animation's state for drawing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimView {
    pub clip: String,
    /// Seconds into the clip at this tick.
    pub time: f32,
    /// Seconds of clip per second of simulation (for interpolation between ticks).
    pub rate: f32,
    pub looped: bool,
}

/// One drawn entity's change: its pose, its look, its animation, or any of these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstanceUpdate {
    pub id: u64,
    pub pose: Option<Pose>,
    pub look: Option<Look>,
    #[serde(default)]
    pub anim: Option<AnimView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightKindView {
    Directional,
    Point,
    Spot,
}

/// A light, posed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightView {
    pub id: u64,
    pub kind: LightKindView,
    pub position: [f32; 3],
    /// The direction it shines (the entity's -z).
    pub direction: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
    pub inner_deg: f32,
    pub outer_deg: f32,
    pub shadows: bool,
}

/// A camera, posed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraView {
    pub id: u64,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub fov_deg: f32,
    pub near: f32,
    pub far: f32,
    pub active: bool,
    pub exposure_ev: f32,
}

/// The environment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentView {
    /// 0 atmosphere, 1 color.
    pub sky: u32,
    pub sky_color: [f32; 3],
    pub ambient: f32,
    pub baked_gi: String,
    /// Project-relative neural GI asset; the renderer uses this before `baked_gi` when present.
    pub neural_gi: String,
    pub gi_intensity: f32,
    pub fog_density: f32,
    pub fog_color: [f32; 3],
    pub exposure_ev: f32,
    pub bloom: f32,
}

/// One wave of the sea, as amoris-physics computes it (a sine on the surface, deep-water speed).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaveView {
    pub length: f32,
    pub height: f32,
    pub toward_deg: f32,
    pub phase: f32,
}

/// The sea the boats float on, drawn to the horizon from the same waves buoyancy uses, and the
/// wind sea and colour drawn only (water.md 60).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeaView {
    pub level: f32,
    pub waves: Vec<WaveView>,
    pub wind: Option<WindSeaView>,
    pub look: Option<SeaLookView>,
}

/// The wind sea: the wind's speed (m/s, at 10 m), the bearing it blows toward, and the fetch.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindSeaView {
    pub speed: f32,
    pub toward_deg: f32,
    pub fetch_km: f32,
}

/// The sea's optical depth, as a water region's look gives it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeaLookView {
    pub fog: [f32; 4],
    pub absorb: [f32; 3],
    pub scatter: [f32; 3],
}

/// A splat cloud, posed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplatView {
    pub id: u64,
    pub asset: String,
    pub pose: Pose,
    pub visible: bool,
}

/// One UI element to draw this frame (text or bar), anchored on the screen or at a world point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiView {
    pub id: u64,
    /// The text; empty for a bar.
    pub text: String,
    /// Bar fill 0..1 (bars only).
    pub fill: f32,
    pub bar: bool,
    pub size: [f32; 2],
    pub color: [f32; 4],
    pub back: [f32; 4],
    /// 0 top-left .. 8 bottom-right (row-major 3x3), 9: at `world`.
    pub anchor: u32,
    pub offset: [f32; 2],
    pub world: [f32; 3],
}

/// A particle emitter, posed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmitterView {
    pub id: u64,
    pub position: [f32; 3],
    /// The entity's +y (the emission axis).
    pub up: [f32; 3],
    pub rate: f32,
    pub burst: u32,
    pub burst_id: u32,
    pub emitting: bool,
    pub lifetime: f32,
    pub speed: f32,
    pub spread_deg: f32,
    pub acceleration: [f32; 3],
    pub drag: f32,
    pub size: [f32; 2],
    pub color_start: [f32; 4],
    pub color_end: [f32; 4],
    pub radius: f32,
}

/// A sound source, posed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioView {
    pub id: u64,
    pub clip: String,
    pub volume: f32,
    pub pitch: f32,
    pub looped: bool,
    pub playing: bool,
    pub spatial: bool,
    pub position: [f32; 3],
}

/// How a block draws (voxels.md 7.1): `shape` 0 a cube (`faces`: top, side, bottom), 1 two
/// crossed quads (`faces[0]`), 2 a liquid (`faces[0]`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BlockLook {
    pub shape: u8,
    pub faces: [String; 3],
    /// A liquid's view from inside: linear colour and fog density per metre (liquids.md 6).
    #[serde(default)]
    pub fog: Option<[f32; 4]>,
    /// A liquid's splash absorption per metre, linear rgb (splashes.md 2); none: no splashes.
    #[serde(default)]
    pub splash: Option<[f32; 3]>,
}

/// One chunk's cells (4096 palette indices, x fastest, then z, then y), or `None`: all air now;
/// and its state bytes (a liquid's level, liquids.md 2), `None` while all are 0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChunkUpdate {
    pub key: [i32; 3],
    pub cells: Option<Vec<u8>>,
    #[serde(default)]
    pub states: Option<Vec<u8>>,
}

/// A block volume's change (voxels.md 7.1): where it is, its palette's looks when they changed,
/// and the chunks that changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoxelUpdate {
    pub id: u64,
    pub origin: [f32; 3],
    pub cell_m: f32,
    pub blocks: Option<Vec<BlockLook>>,
    pub chunks: Vec<ChunkUpdate>,
}

/// How a water region looks (water.md 9): the view from under its surface (linear colour and fog
/// density per metre), its absorption and scattering per metre (linear rgb), its style: `blocky`
/// draws the surface flat over each block of `per` x `per` columns, and the still water drawn
/// around it, metres (0: none).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WaterLookView {
    pub fog: [f32; 4],
    pub absorb: [f32; 3],
    pub scatter: [f32; 3],
    pub blocky: bool,
    pub per: u32,
    pub surround_m: f32,
    /// The sides (-x, +x, -z, +z) open on the sea, where the region's water meets the sea's.
    pub sea: [bool; 4],
    /// Seconds the bed stays damp after the water leaves it (0: the bed as it is).
    pub wet_s: f32,
}

/// One tile of a water region (16 x 16 columns, x fastest): each column's surface (world y), depth
/// and velocity (m/s along x and z).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaterTileView {
    pub key: [u32; 2],
    pub columns: Vec<[f32; 4]>,
}

/// A water region's change (water.md 9): its corner (world x, z), column size and column counts,
/// its look when it changed, and its tiles: every tile in its first frame and a reset frame, then
/// the tiles stepped since.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaterUpdate {
    pub id: u64,
    pub origin: [f32; 2],
    pub cell_m: f32,
    pub size: [u32; 2],
    pub look: Option<WaterLookView>,
    pub tiles: Vec<WaterTileView>,
}

/// A resident field's presentation program. The checkpoint travels only on first publication,
/// reset or compaction; subsequent updates carry only a checked suffix and latest GPU digest.
/// A viewport reconstructs its own visual cache, without reading that cache into the game.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResidentWaterUpdate {
    pub id: u64,
    pub origin: [f32; 2],
    pub cell_m: f32,
    pub look: WaterLookView,
    #[serde(with = "checkpoint_feed")]
    pub checkpoint: Option<Arc<crate::water_field::CheckedCheckpoint>>,
    pub recipe: crate::water_field::FieldPatch,
    pub edge_level: Option<f32>,
}

/// A body's point that stirs the water it floats or swims in (water.md 57): a Floater's buoyancy
/// point, a swimmer's body, or a swimmer's or paddler's hand. The renderer measures its immersion
/// against the drawn surface and makes ripples from its motion relative to the water.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StirView {
    /// The entity, and the point within it, so the renderer follows each point from tick to tick.
    pub id: u64,
    pub point: u32,
    /// The point's centre, world metres.
    pub at: [f32; 3],
    /// Its vertical extent (metres) and its volume (cubic metres): it stands for a waterplane of
    /// `volume / size` square metres.
    pub size: f32,
    pub volume: f32,
}

/// The most stirring points a frame carries; past it, in entity order, points make no ripples.
pub const MAX_STIRS: usize = 4096;

/// What changed in the visual state since the subscriber's last frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RenderFrame {
    /// The tick this state is the end of.
    pub tick: u64,
    /// Simulated seconds at that tick.
    pub t_s: f64,
    /// The simulation's tick length, seconds (for interpolation).
    pub dt_s: f64,
    /// Everything below replaces the renderer's scene instead of updating it.
    pub reset: bool,
    pub instances: Vec<InstanceUpdate>,
    pub removed: Vec<u64>,
    /// The full list, when any light changed.
    pub lights: Option<Vec<LightView>>,
    /// The full list, when any camera changed.
    pub cameras: Option<Vec<CameraView>>,
    pub environment: Option<EnvironmentView>,
    /// `Some(None)`: the sea was removed.
    pub sea: Option<Option<SeaView>>,
    /// The full list, when any splat changed.
    pub splats: Option<Vec<SplatView>>,
    /// The full list of UI elements, when any changed (or an entity carrying one moved).
    #[serde(default)]
    pub ui: Option<Vec<UiView>>,
    /// The full list of sound sources, when any changed or moved.
    #[serde(default)]
    pub audio: Option<Vec<AudioView>>,
    /// The full list of particle emitters, when any changed or moved.
    #[serde(default)]
    pub emitters: Option<Vec<EmitterView>>,
    /// Block volumes that changed (voxels.md 7.1).
    #[serde(default)]
    pub voxels: Vec<VoxelUpdate>,
    /// Block volumes gone.
    #[serde(default)]
    pub voxels_removed: Vec<u64>,
    /// Water regions that changed (water.md 9).
    #[serde(default)]
    pub water: Vec<WaterUpdate>,
    #[serde(default)]
    pub resident_water: Vec<ResidentWaterUpdate>,
    /// Water regions gone.
    #[serde(default)]
    pub water_removed: Vec<u64>,
    /// The full list of points stirring the water regions, when any of them moved.
    #[serde(default)]
    pub stirs: Option<Vec<StirView>>,
}

impl RenderFrame {
    /// Whether it carries nothing but the clock.
    pub fn is_empty(&self) -> bool {
        !self.reset
            && self.instances.is_empty()
            && self.removed.is_empty()
            && self.lights.is_none()
            && self.cameras.is_none()
            && self.environment.is_none()
            && self.sea.is_none()
            && self.splats.is_none()
            && self.ui.is_none()
            && self.audio.is_none()
            && self.emitters.is_none()
            && self.voxels.is_empty()
            && self.voxels_removed.is_empty()
            && self.water.is_empty()
            && self.resident_water.is_empty()
            && self.water_removed.is_empty()
            && self.stirs.is_none()
    }

    /// Folds `next` (a later frame) into this one, as if both had been applied in order.
    pub fn merge(&mut self, next: RenderFrame) -> Result<(), String> {
        next.validate_resident()?;
        // Preflight every resident chain before changing any frame fields. Each source frame
        // has at most one update per region, so the checked bases stay unchanged until append.
        if !next.reset {
            for w in &next.resident_water {
                if let Some(cur) = self.resident_water.iter().find(|c| c.id == w.id) {
                    cur.recipe.check_append(&w.recipe)?;
                    if w.checkpoint.is_none() && cur.recipe.checkpoint != w.recipe.checkpoint {
                        return Err("resident visual checkpoint changed without its seed".into());
                    }
                }
            }
        }
        self.tick = next.tick;
        self.t_s = next.t_s;
        self.dt_s = next.dt_s;
        if next.reset {
            *self = next;
            return Ok(());
        }
        if !next.removed.is_empty() {
            let gone: std::collections::HashSet<u64> = next.removed.iter().copied().collect();
            self.instances.retain(|u| !gone.contains(&u.id));
            self.removed.extend(next.removed);
        }
        if !next.instances.is_empty() {
            if self.instances.is_empty() {
                self.instances = next.instances;
            } else {
                let mut at: HashMap<u64, usize> = self
                    .instances
                    .iter()
                    .enumerate()
                    .map(|(i, u)| (u.id, i))
                    .collect();
                for u in next.instances {
                    // An entity removed and then spawned again under the same id cannot happen:
                    // ids are never reused (simulation.md 7).
                    match at.get(&u.id) {
                        Some(&i) => {
                            let cur = &mut self.instances[i];
                            if u.pose.is_some() {
                                cur.pose = u.pose;
                            }
                            if u.look.is_some() {
                                cur.look = u.look;
                            }
                            if u.anim.is_some() {
                                cur.anim = u.anim;
                            }
                        }
                        None => {
                            at.insert(u.id, self.instances.len());
                            self.instances.push(u);
                        }
                    }
                }
            }
        }
        if next.lights.is_some() {
            self.lights = next.lights;
        }
        if next.cameras.is_some() {
            self.cameras = next.cameras;
        }
        if next.environment.is_some() {
            self.environment = next.environment;
        }
        if next.sea.is_some() {
            self.sea = next.sea;
        }
        if next.splats.is_some() {
            self.splats = next.splats;
        }
        if next.ui.is_some() {
            self.ui = next.ui;
        }
        if next.audio.is_some() {
            self.audio = next.audio;
        }
        if next.emitters.is_some() {
            self.emitters = next.emitters;
        }
        if next.stirs.is_some() {
            self.stirs = next.stirs;
        }
        if !next.voxels_removed.is_empty() {
            self.voxels.retain(|v| !next.voxels_removed.contains(&v.id));
            self.voxels_removed.extend(next.voxels_removed);
        }
        for v in next.voxels {
            self.voxels_removed.retain(|&id| id != v.id);
            match self.voxels.iter_mut().find(|o| o.id == v.id) {
                Some(cur) => {
                    cur.origin = v.origin;
                    cur.cell_m = v.cell_m;
                    if v.blocks.is_some() {
                        cur.blocks = v.blocks;
                    }
                    for c in v.chunks {
                        match cur.chunks.iter_mut().find(|o| o.key == c.key) {
                            Some(o) => *o = c,
                            None => cur.chunks.push(c),
                        }
                    }
                }
                None => self.voxels.push(v),
            }
        }
        if !next.water_removed.is_empty() {
            self.water.retain(|w| !next.water_removed.contains(&w.id));
            self.resident_water
                .retain(|w| !next.water_removed.contains(&w.id));
            self.water_removed.extend(next.water_removed);
        }
        for w in next.water {
            self.resident_water.retain(|o| o.id != w.id);
            self.water_removed.retain(|&id| id != w.id);
            match self.water.iter_mut().find(|o| o.id == w.id) {
                Some(cur) => {
                    let resized = cur.size != w.size;
                    (cur.origin, cur.cell_m, cur.size) = (w.origin, w.cell_m, w.size);
                    if w.look.is_some() {
                        cur.look = w.look;
                    }
                    if resized {
                        cur.tiles = w.tiles;
                        continue;
                    }
                    for t in w.tiles {
                        match cur.tiles.iter_mut().find(|o| o.key == t.key) {
                            Some(o) => *o = t,
                            None => cur.tiles.push(t),
                        }
                    }
                }
                None => self.water.push(w),
            }
        }
        for mut w in next.resident_water {
            self.water_removed.retain(|&id| id != w.id);
            self.water.retain(|o| o.id != w.id);
            if let Some(cur) = self.resident_water.iter_mut().find(|o| o.id == w.id) {
                if w.checkpoint.is_none() && cur.recipe.checkpoint == w.recipe.checkpoint {
                    w.checkpoint = cur.checkpoint.take();
                }
                if w.recipe.start != 0 {
                    let mut old = std::mem::replace(cur, w);
                    std::mem::swap(&mut cur.recipe, &mut old.recipe);
                    cur.recipe.append_validated(old.recipe);
                } else {
                    *cur = w;
                }
            } else {
                self.resident_water.push(w);
            }
        }
        Ok(())
    }

    fn validate_resident(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        for w in &self.resident_water {
            if !ids.insert(w.id) {
                return Err("render frame repeats a resident region".into());
            }
            if self.water_removed.contains(&w.id) || self.water.iter().any(|c| c.id == w.id) {
                return Err(
                    "render frame conflicts over a resident region's presence or representation"
                        .into(),
                );
            }
            w.recipe.validate()?;
            if self.reset && (w.checkpoint.is_none() || w.recipe.start != 0) {
                return Err("resident reset frame is missing its full program/seed".into());
            }
            if let Some(c) = &w.checkpoint
                && c.key() != w.recipe.checkpoint
            {
                return Err("resident visual checkpoint identity mismatch".into());
            }
        }
        Ok(())
    }

    /// The bytes a viewport receives (over `/render`, host-protocol.md 5, or from a game worker):
    /// [`FORMAT`] in little endian, then the frame in bincode 2's standard configuration.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = FORMAT.to_le_bytes().to_vec();
        bincode2::serde::encode_into_std_write(self, &mut out, bincode2::config::standard())
            .unwrap_or_default();
        out
    }

    /// The frame, or why these bytes are not one of this build's frames.
    pub fn decode(bytes: &[u8]) -> Result<RenderFrame, String> {
        let (format, body) = bytes
            .split_first_chunk::<8>()
            .ok_or_else(|| format!("a render frame of {} bytes has no format", bytes.len()))?;
        let format = u64::from_le_bytes(*format);
        if format != FORMAT {
            return Err(format!(
                "render frame format {format:016x} is not this build's {FORMAT:016x}: the sender and the \
                 viewport were built from different revisions of amoris-assets (rebuild the viewport)"
            ));
        }
        let (frame, used): (Self, usize) =
            bincode2::serde::decode_from_slice(body, bincode2::config::standard())
                .map_err(|e| format!("render frame: {e}"))?;
        if used != body.len() {
            return Err("render frame has trailing bytes".into());
        }
        frame.validate_resident()?;
        Ok(frame)
    }
}

/// The feed's wire format fingerprints its type and checkpoint-codec sources. A viewport built
/// from another revision refuses the bytes instead of misreading them; both sides rebuild together.
pub const FORMAT: u64 = fnv1a(include_bytes!("frame.rs"))
    ^ fnv1a(include_bytes!("visual.rs")).rotate_left(1)
    ^ fnv1a(include_bytes!("water_field.rs")).rotate_left(2)
    ^ fnv1a(include_bytes!("checkpoint_feed.rs")).rotate_left(3);

const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

/// One subscriber's pending frame.
#[derive(Default)]
pub struct Mailbox {
    pending: Mutex<RenderFrame>,
    /// The subscriber has nothing yet (or lost its scene): the next extraction sends it a reset.
    wants_reset: AtomicBool,
    closed: AtomicBool,
}

impl Mailbox {
    /// The frame merged from everything pushed since the last take.
    pub fn take(&self) -> RenderFrame {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Asks for a full frame at the next extraction.
    pub fn request_reset(&self) {
        self.wants_reset.store(true, Ordering::Release);
    }

    /// Stops deliveries; the feed forgets the mailbox.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    fn push(&self, frame: RenderFrame) {
        let mut p = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        if p.is_empty() && !p.reset {
            frame
                .validate_resident()
                .expect("producer render reset/patch is valid");
            *p = frame;
        } else {
            p.merge(frame)
                .expect("producer render updates form a continuous resident program");
        }
    }
}

/// The game's render feed: its subscribers.
#[derive(Clone, Default)]
pub struct Feed {
    subs: Arc<Mutex<Vec<Arc<Mailbox>>>>,
}

impl Feed {
    pub fn new() -> Feed {
        Feed::default()
    }

    /// A new subscriber; its first frame is a reset.
    pub fn subscribe(&self) -> Arc<Mailbox> {
        let m = Arc::new(Mailbox::default());
        m.request_reset();
        self.subs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(m.clone());
        m
    }

    /// Whether anyone listens.
    pub fn has_subscribers(&self) -> bool {
        let mut subs = self.subs.lock().unwrap_or_else(|p| p.into_inner());
        subs.retain(|m| !m.closed.load(Ordering::Acquire));
        !subs.is_empty()
    }

    /// Whether a subscriber waits for a full frame.
    pub fn wants_reset(&self) -> bool {
        self.subs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|m| m.wants_reset.load(Ordering::Acquire))
    }

    /// Delivers a tick's frames: `delta` to subscribers that have a scene, `full` (when made) to
    /// those that asked for a reset. Every subscriber needs a reset after `reset_all`.
    pub fn publish(&self, delta: RenderFrame, full: Option<RenderFrame>) {
        let subs = self.subs.lock().unwrap_or_else(|p| p.into_inner());
        for m in subs.iter() {
            if m.closed.load(Ordering::Acquire) {
                continue;
            }
            if m.wants_reset.load(Ordering::Acquire) {
                if let Some(f) = &full {
                    m.wants_reset.store(false, Ordering::Release);
                    m.push(f.clone());
                }
            } else if !delta.is_empty() || delta.reset {
                m.push(delta.clone());
            } else {
                // Keep the clock moving for interpolation even when nothing changed.
                let mut p = m.pending.lock().unwrap_or_else(|p| p.into_inner());
                p.tick = delta.tick;
                p.t_s = delta.t_s;
                p.dt_s = delta.dt_s;
            }
        }
    }

    /// Every subscriber's scene is stale (a restore, Play or Stop replaced the world).
    pub fn reset_all(&self) {
        for m in self.subs.lock().unwrap_or_else(|p| p.into_inner()).iter() {
            m.request_reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(id: u64, x: f32) -> InstanceUpdate {
        InstanceUpdate {
            id,
            pose: Some(Pose {
                position: [x, 0.0, 0.0],
                ..Pose::default()
            }),
            look: None,
            anim: None,
        }
    }

    #[test]
    fn merge_keeps_the_latest_pose_and_drops_removed() {
        let mut a = RenderFrame {
            instances: vec![up(1, 1.0), up(2, 2.0)],
            ..RenderFrame::default()
        };
        a.merge(RenderFrame {
            tick: 2,
            instances: vec![up(1, 5.0), up(3, 3.0)],
            removed: vec![2],
            ..RenderFrame::default()
        })
        .unwrap();
        assert_eq!(a.tick, 2);
        assert_eq!(a.instances.len(), 2);
        assert_eq!(a.instances[0].pose.unwrap().position[0], 5.0);
        assert_eq!(a.removed, vec![2]);
    }

    #[test]
    fn merged_water_keeps_the_latest_tile_of_each_key() {
        let tile = |key: [u32; 2], x: f32| WaterTileView {
            key,
            columns: vec![[x; 4]; 256],
        };
        let water = |tiles: Vec<WaterTileView>, look: bool| WaterUpdate {
            id: 9,
            origin: [0.0; 2],
            cell_m: 0.5,
            size: [32, 32],
            look: look.then(WaterLookView::default),
            tiles,
        };
        let mut a = RenderFrame {
            water: vec![water(vec![tile([0, 0], 1.0), tile([1, 0], 1.0)], true)],
            ..RenderFrame::default()
        };
        a.merge(RenderFrame {
            water: vec![water(vec![tile([1, 0], 2.0), tile([1, 1], 2.0)], false)],
            ..RenderFrame::default()
        })
        .unwrap();
        let w = &a.water[0];
        assert!(w.look.is_some(), "the earlier look stays");
        let x: Vec<([u32; 2], f32)> = w.tiles.iter().map(|t| (t.key, t.columns[0][0])).collect();
        assert_eq!(x, vec![([0, 0], 1.0), ([1, 0], 2.0), ([1, 1], 2.0)]);
        a.merge(RenderFrame {
            water_removed: vec![9],
            ..RenderFrame::default()
        })
        .unwrap();
        assert!(a.water.is_empty() && a.water_removed == vec![9]);
    }

    #[test]
    fn environment_baked_gi_survives_frame_transport() {
        let frame = RenderFrame {
            environment: Some(EnvironmentView {
                sky: 1,
                sky_color: [0.0; 3],
                ambient: 0.0,
                baked_gi: "lighting/room.gi.json".into(),
                neural_gi: "lighting/room.neural-gi.json".into(),
                gi_intensity: 0.75,
                fog_density: 0.0,
                fog_color: [0.0; 3],
                exposure_ev: 0.0,
                bloom: 0.0,
            }),
            ..RenderFrame::default()
        };
        assert_eq!(RenderFrame::decode(&frame.encode()), Ok(frame));
    }

    #[test]
    fn a_new_subscriber_gets_the_full_frame_then_deltas() {
        let feed = Feed::new();
        let m = feed.subscribe();
        assert!(feed.wants_reset());
        let full = RenderFrame {
            reset: true,
            instances: vec![up(1, 1.0)],
            ..RenderFrame::default()
        };
        feed.publish(RenderFrame::default(), Some(full));
        let f = m.take();
        assert!(f.reset && f.instances.len() == 1);
        feed.publish(
            RenderFrame {
                instances: vec![up(1, 2.0)],
                ..RenderFrame::default()
            },
            None,
        );
        let f = m.take();
        assert!(!f.reset && f.instances[0].pose.unwrap().position[0] == 2.0);
        let mut bytes = f.encode();
        assert_eq!(RenderFrame::decode(&bytes), Ok(f));
        bytes[0] ^= 1;
        assert!(
            RenderFrame::decode(&bytes)
                .unwrap_err()
                .contains("rebuild the viewport")
        );
    }
}
