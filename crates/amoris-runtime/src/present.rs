//! The render feed's producer (charter 4.4; amoris-assets `frame`): after a tick or a boundary
//! that wrote, the game thread extracts what changed in the visual state into a `RenderFrame` and
//! hands it to the feed's subscribers. Presentation only: change detection is read here and
//! nowhere in the simulation (amoris-sim's rule), and nothing here writes a component.
//!
//! A frame carries the instances whose `Transform` or `Model` changed since the last extraction,
//! the entities that lost their `Model` or were despawned, and the full lists of lights, cameras,
//! splats, the environment and the sea when any of them changed. A new subscriber, a restore, a
//! fork swapped in (Play, Stop) or a reload gets a full frame.

// The determinism lists (clippy.toml) are the tick code's: this extraction reads change detection
// and keeps a map of what it drew, which never reaches the world, its hash or a replay.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::collections::HashMap;

use amoris_assets::frame::{
    AnimView, AudioView, BlockLook, CameraView, ChunkUpdate, EmitterView, EnvironmentView,
    InstanceUpdate, LightKindView, LightView, Look, Pose, RainView, RenderFrame,
    ResidentWaterUpdate, SeaLookView, SeaView, SplatView, StirView, TerrainLayerView, TerrainView,
    UiView, VoxelUpdate, WaterLookView, WaterTileView, WaterUpdate, WaveView, WeatherView,
    WindSeaView,
};
use amoris_assets::{
    Animator, AudioSource, Camera, Environment, Feed, Light, LightKind, Model, ParticleEmitter,
    SkyKind, Splat, UiAnchor, UiBar, UiText,
};
use amoris_physics::water::{
    Edge, Solver,
    resident::{WaterCheckpoints, WaterResident},
};
use amoris_physics::{
    BedSource, Rain, Sea, Style, Terrain, Transform, WaterRegion, WaterSteps, Wind,
};
use amoris_sim::voxels::{BlockShape, Voxels};
use amoris_sim::{ContentHash, EntityId, EntityIndex, SimClock};
use bevy_ecs::change_detection::DetectChanges;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::{Or, With, World};
use bevy_ecs::query::QueryState;
use bevy_ecs::world::{Ref, WorldId};

/// Extracts render frames from one game's world.
#[derive(Default)]
pub struct Extractor {
    /// The world and generation the last frame described; another one means a full frame.
    seen: Option<(WorldId, u64)>,
    /// Entities currently drawn, so a despawn (whose id is gone with the entity) can be reported.
    drawn: HashMap<Entity, u64>,
    /// The delta's queries, kept for the world they were made in.
    queries: Option<Queries>,
    /// Each block volume as last sent: what a change is compared with (voxels.md 7.1).
    volumes: HashMap<Entity, SentVolume>,
    /// Each water region as last sent (water.md 9).
    waters: HashMap<Entity, SentWater>,
    /// Each terrain's id, as last sent (water.md 65.1).
    terrains: HashMap<Entity, u64>,
    /// The points stirring the water as last sent (water.md 57).
    stirs: Vec<StirView>,
}

/// A water region as the subscribers last received it: its id, the tick it was sent at (its tiles
/// stepped later are sent next), and its look.
struct SentWater {
    id: u64,
    tick: u64,
    look: WaterLookView,
    checkpoint: Option<ContentHash>,
    program: Option<ContentHash>,
    steps: usize,
    environment: Option<String>,
}

fn water_look(r: &WaterRegion) -> WaterLookView {
    WaterLookView {
        fog: r.look.fog,
        absorb: r.look.absorb,
        scatter: r.look.scatter,
        blocky: r.look.style == Style::Blocky,
        surround_m: r.look.surround_m,
        sea: r.edges.map(|e| e == Edge::Sea),
        joined: r.edges.map(|e| matches!(e, Edge::Joined { .. })),
        wet_s: r.look.wet_s,
        per: match r.bed {
            BedSource::Voxels { per, .. } => per,
            BedSource::Colliders { .. } | BedSource::Terrain { .. } => 1,
        },
    }
}

/// A terrain as drawn: its heights and layers (water.md 65.1).
fn terrain_view(id: u64, t: &Terrain, origin: [f64; 3]) -> TerrainView {
    TerrainView {
        id,
        origin: f3(origin),
        cell_m: f(t.cell_m),
        size: t.size,
        heights: t.heights().into_iter().map(f).collect(),
        layers: t
            .look
            .layers
            .iter()
            .map(|l| TerrainLayerView {
                material: l.material.clone(),
                color: l.color.map(f),
                roughness: f(l.roughness),
                steeper_than: l.steeper_than.map(f),
                below_m: l.below_m.map(f),
            })
            .collect(),
    }
}

/// A region without tiles over a terrain, as it will start (water.md 65.2): the terrain's beds with
/// the region's fills. None for another bed, or no terrain.
fn preview(world: &World, r: &WaterRegion, corner: [f64; 2]) -> Option<WaterRegion> {
    let BedSource::Terrain { terrain } = r.bed else {
        return None;
    };
    let e = world.get_resource::<EntityIndex>()?.get(terrain)?;
    let ground = world.get::<Terrain>(e)?;
    let at = world.get::<Transform>(e).map_or([0.0; 3], |t| t.position);
    let mut p = r.clone();
    amoris_physics::water::author::from_terrain(&mut p, corner, ground, at);
    Some(p)
}

/// The update for a region the world has not stepped (none yet, or a resident one before its
/// first step, as the paused edit world holds it): its tiles, authored or previewed.
fn unstepped(
    world: &World,
    sent: &mut Option<SentWater>,
    id: u64,
    r: &WaterRegion,
    p: [f64; 3],
    tick: u64,
    marks: Option<&Vec<u64>>,
) -> Option<WaterUpdate> {
    let origin = [f(p[0]), f(p[2])];
    if r.well_formed() || r.solver != Solver::GpuResident {
        return Some(water_update(sent, id, r, origin, tick, marks));
    }
    let shown = preview(world, r, [p[0], p[2]])?;
    Some(water_update(sent, id, &shown, origin, tick, None))
}

/// Tile `n`'s columns: surface, depth, velocity.
fn water_tile(r: &WaterRegion, n: usize) -> WaterTileView {
    let t = &r.tiles[n];
    let tx = r.tiles_along()[0];
    let (bx, bz) = ((n % tx) * 16, (n / tx) * 16);
    let columns = (0..256)
        .map(|c| {
            let (i, k) = (bx + c % 16, bz + c / 16);
            let v = r.tile_velocity(i, k);
            [f(t.bed[c] + t.depth[c]), f(t.depth[c]), f(v[0]), f(v[1])]
        })
        .collect();
    WaterTileView {
        key: t.key,
        columns,
    }
}

/// The update that brings `sent` (none: nothing sent yet, so every tile) to `r` at `tick`, given
/// the tick each tile was last stepped (`marks`; none: every tile); `sent` is brought up to date.
fn water_update(
    sent: &mut Option<SentWater>,
    id: u64,
    r: &WaterRegion,
    origin: [f32; 2],
    tick: u64,
    marks: Option<&Vec<u64>>,
) -> WaterUpdate {
    let look = water_look(r);
    let tiles: Vec<WaterTileView> = if !r.well_formed() {
        Vec::new()
    } else {
        match (sent.as_ref(), marks) {
            (Some(s), Some(m)) => (0..r.tiles.len())
                .filter(|&n| m.get(n).is_some_and(|&t| t > s.tick))
                .map(|n| water_tile(r, n))
                .collect(),
            _ => (0..r.tiles.len()).map(|n| water_tile(r, n)).collect(),
        }
    };
    let new_look = sent.as_ref().is_none_or(|s| s.look != look);
    *sent = Some(SentWater {
        id,
        tick,
        look: look.clone(),
        checkpoint: None,
        program: None,
        steps: 0,
        environment: None,
    });
    WaterUpdate {
        id,
        origin,
        cell_m: f(r.cell_m),
        size: r.size,
        look: new_look.then_some(look),
        tiles,
    }
}

fn resident_update(
    sent: &mut Option<SentWater>,
    id: u64,
    r: &WaterRegion,
    state: &WaterResident,
    checkpoints: &WaterCheckpoints,
    origin: [f32; 2],
    tick: u64,
) -> ResidentWaterUpdate {
    let look = water_look(r);
    let key = state.recipe.checkpoint();
    let previous = sent
        .as_ref()
        .filter(|s| {
            s.checkpoint == Some(key) && s.environment.as_ref() == Some(&state.recipe.environment)
        })
        .and_then(|s| s.program.map(|p| (s.steps, p)));
    let patch = state.recipe.patch_after(previous);
    let checkpoint = sent
        .as_ref()
        .is_none_or(|s| s.checkpoint != Some(key))
        .then(|| {
            checkpoints
                .fields
                .get(&key)
                .expect("published resident checkpoint")
                .clone()
        });
    *sent = Some(SentWater {
        id,
        tick,
        look: look.clone(),
        checkpoint: Some(key),
        program: Some(patch.program),
        steps: patch.end(),
        environment: Some(patch.environment.clone()),
    });
    ResidentWaterUpdate {
        id,
        origin,
        cell_m: f(r.cell_m),
        look,
        checkpoint,
        recipe: patch,
        edge_level: (state.stats.wet_edge_count > 0).then(|| f(state.stats.wet_edge_surface)),
    }
}

/// A block volume as the subscribers last received it.
struct SentVolume {
    id: u64,
    origin: [f32; 3],
    cell_m: f32,
    blocks: Vec<BlockLook>,
    chunks: HashMap<[i32; 3], SentChunk>,
}

/// A chunk's cells and states as last sent.
type SentChunk = (Box<[u8]>, Option<Box<[u8]>>);

fn block_looks(v: &Voxels) -> Vec<BlockLook> {
    v.palette
        .iter()
        .map(|b| BlockLook {
            shape: match b.shape {
                BlockShape::Cube => 0,
                BlockShape::Cross => 1,
                BlockShape::Liquid => 2,
            },
            faces: b.faces.clone(),
            fog: b.flow.map(|f| f.fog.map(f32_of)),
        })
        .collect()
}

/// The update that brings `sent` (none: nothing sent yet) to `v` at `origin`, or `None` when
/// nothing changed; `sent` is brought up to date.
fn volume_update(
    sent: &mut Option<SentVolume>,
    id: u64,
    v: &Voxels,
    origin: [f32; 3],
) -> Option<VoxelUpdate> {
    let blocks = block_looks(v);
    let cell_m = f(v.cell_m);
    let s = sent.get_or_insert_with(|| SentVolume {
        id,
        origin,
        cell_m,
        blocks: Vec::new(),
        chunks: HashMap::new(),
    });
    let mut update = VoxelUpdate {
        id,
        origin,
        cell_m,
        blocks: None,
        chunks: Vec::new(),
    };
    let mut changed = s.origin != origin || s.cell_m != cell_m;
    if s.blocks != blocks {
        // New looks redraw every chunk; the renderer remeshes them all.
        s.blocks = blocks.clone();
        update.blocks = Some(blocks);
        changed = true;
    }
    (s.origin, s.cell_m) = (origin, cell_m);
    let mut present = std::collections::HashSet::with_capacity(v.chunks.len());
    for c in &v.chunks {
        present.insert(c.key);
        let states = c.states.as_ref().map(|st| &st.0);
        let same = s.chunks.get(&c.key).is_some_and(|(cells, old)| {
            **cells == *c.cells.0 && old.as_deref() == states.map(|b| &**b)
        });
        if same {
            continue;
        }
        s.chunks.insert(c.key, (c.cells.0.clone(), states.cloned()));
        update.chunks.push(ChunkUpdate {
            key: c.key,
            cells: Some(c.cells.0.to_vec()),
            states: states.map(|b| b.to_vec()),
        });
    }
    let mut gone: Vec<[i32; 3]> = s
        .chunks
        .keys()
        .filter(|k| !present.contains(*k))
        .copied()
        .collect();
    gone.sort_unstable();
    for key in gone {
        s.chunks.remove(&key);
        update.chunks.push(ChunkUpdate {
            key,
            cells: None,
            states: None,
        });
    }
    (changed || !update.chunks.is_empty()).then_some(update)
}

type Opt<T> = Option<Ref<'static, T>>;
type InstanceRow = (
    Entity,
    &'static EntityId,
    Ref<'static, Transform>,
    Ref<'static, Model>,
    Opt<Animator>,
);
type UiRow = (Opt<Transform>, Opt<UiText>, Opt<UiBar>);
type UiFilter = Or<(With<UiText>, With<UiBar>)>;

/// What a delta reads each time: the drawn instances and whether each list changed. Kept between
/// extractions (a query made again every tick cost about a microsecond each).
type WaterRow = (
    Entity,
    &'static EntityId,
    Ref<'static, WaterRegion>,
    Opt<Transform>,
    Opt<WaterResident>,
);

struct Queries {
    world: WorldId,
    instances: QueryState<InstanceRow>,
    lights: QueryState<(Ref<'static, Transform>, Ref<'static, Light>)>,
    cameras: QueryState<(Ref<'static, Transform>, Ref<'static, Camera>)>,
    environment: QueryState<Ref<'static, Environment>>,
    sea: QueryState<Ref<'static, Sea>>,
    wind: QueryState<Ref<'static, Wind>>,
    rain: QueryState<Ref<'static, Rain>>,
    regions: QueryState<Ref<'static, WaterRegion>>,
    emitters: QueryState<(Opt<Transform>, Ref<'static, ParticleEmitter>)>,
    audio: QueryState<(Opt<Transform>, Ref<'static, AudioSource>)>,
    ui: QueryState<UiRow, UiFilter>,
    splats: QueryState<(Ref<'static, Transform>, Ref<'static, Splat>)>,
    voxels: QueryState<(
        Entity,
        &'static EntityId,
        Ref<'static, Voxels>,
        Opt<Transform>,
    )>,
    water: QueryState<WaterRow>,
    terrain: QueryState<(
        Entity,
        &'static EntityId,
        Ref<'static, Terrain>,
        Opt<Transform>,
    )>,
}

impl Queries {
    fn new(world: &mut World) -> Queries {
        Queries {
            world: world.id(),
            instances: QueryState::new(world),
            lights: QueryState::new(world),
            cameras: QueryState::new(world),
            environment: QueryState::new(world),
            sea: QueryState::new(world),
            wind: QueryState::new(world),
            rain: QueryState::new(world),
            regions: QueryState::new(world),
            emitters: QueryState::new(world),
            audio: QueryState::new(world),
            ui: QueryState::new(world),
            splats: QueryState::new(world),
            voxels: QueryState::new(world),
            water: QueryState::new(world),
            terrain: QueryState::new(world),
        }
    }
}

fn f(x: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        x as f32
    }
}

fn f32_of(x: f64) -> f32 {
    f(x)
}

fn f3(v: [f64; 3]) -> [f32; 3] {
    [f(v[0]), f(v[1]), f(v[2])]
}

fn pose(t: &Transform, scale: [f64; 3]) -> Pose {
    Pose {
        position: f3(t.position),
        rotation: [
            f(t.rotation[0]),
            f(t.rotation[1]),
            f(t.rotation[2]),
            f(t.rotation[3]),
        ],
        scale: f3(scale),
    }
}

fn anim(a: &Animator) -> AnimView {
    AnimView {
        clip: a.clip.clone(),
        time: f(a.time),
        rate: if a.playing { f(a.speed) } else { 0.0 },
        looped: a.looped,
    }
}

fn look(m: &Model) -> Look {
    Look {
        mesh: m.mesh.clone(),
        material: m.material.clone(),
        color: [f(m.color[0]), f(m.color[1]), f(m.color[2]), f(m.color[3])],
        metallic: f(m.metallic),
        roughness: f(m.roughness),
        transmission: m.transmission.map(f),
        ior: m.ior.map(f),
        emissive: f3(m.emissive),
        cast_shadows: m.cast_shadows,
        visible: m.visible,
    }
}

/// The direction an entity's -z faces.
fn forward(t: &Transform) -> [f32; 3] {
    let q = t.rotation;
    let v = [0.0, 0.0, -1.0];
    let u = [q[0], q[1], q[2]];
    let c = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t2 = c(u, v).map(|x| x * 2.0);
    let r = c(u, t2);
    f3([
        v[0] + q[3] * t2[0] + r[0],
        v[1] + q[3] * t2[1] + r[1],
        v[2] + q[3] * t2[2] + r[2],
    ])
}

impl Extractor {
    pub fn new() -> Extractor {
        Extractor::default()
    }

    /// Delivers this boundary's frames to `feed` (nothing when nobody listens). `generation`
    /// changes whenever the world's content was replaced in place (a restore).
    pub fn publish(&mut self, world: &mut World, generation: u64, feed: &Feed) {
        if !feed.has_subscribers() {
            // The first subscriber gets a full frame; the removal trackers stay bounded.
            self.seen = None;
            world.clear_trackers();
            return;
        }
        let key = (world.id(), generation);
        let replaced = self.seen != Some(key);
        // The delta first: a full frame rebuilds the drawn set the delta reports removals from.
        let (delta, full) = if replaced {
            // Everyone needs the full frame: the old scene is gone.
            feed.reset_all();
            let full = self.full(world);
            let delta = RenderFrame {
                tick: full.tick,
                ..RenderFrame::default()
            };
            (delta, Some(full))
        } else {
            let delta = self.delta(world);
            let full = feed.wants_reset().then(|| self.full(world));
            (delta, full)
        };
        self.seen = Some(key);
        feed.publish(delta, full);
        world.clear_trackers();
    }

    fn clock(world: &World) -> (u64, f64, f64) {
        world
            .get_resource::<SimClock>()
            .map_or((0, 0.0, 1.0 / 60.0), |c| (c.tick.0, c.time(), c.dt()))
    }

    fn full(&mut self, world: &mut World) -> RenderFrame {
        let (tick, t_s, dt_s) = Self::clock(world);
        self.drawn.clear();
        let mut instances = Vec::new();
        let mut q = world.query::<(Entity, &EntityId, &Transform, &Model, Option<&Animator>)>();
        let mut rows: Vec<(u64, Entity, InstanceUpdate)> = q
            .iter(world)
            .map(|(e, id, t, m, a)| {
                (
                    id.get(),
                    e,
                    InstanceUpdate {
                        id: id.get(),
                        pose: Some(pose(t, m.scale)),
                        look: Some(look(m)),
                        anim: a.map(anim),
                    },
                )
            })
            .collect();
        rows.sort_unstable_by_key(|r| r.0);
        for (id, e, u) in rows {
            self.drawn.insert(e, id);
            instances.push(u);
        }
        let mut frame = RenderFrame {
            tick,
            t_s,
            dt_s,
            reset: true,
            instances,
            ..RenderFrame::default()
        };
        frame.lights = Some(Self::lights(world));
        frame.cameras = Some(Self::cameras(world));
        frame.environment =
            Self::environment(world).or_else(|| Some(env_view(&Environment::default())));
        frame.sea = Some(Self::sea(world));
        frame.weather = Some(Self::weather(world));
        frame.splats = Some(Self::splats(world));
        frame.ui = Some(Self::ui(world));
        frame.audio = Some(Self::audio(world));
        frame.emitters = Some(Self::emitters(world));
        self.stirs = crate::stirs::stirs(world);
        frame.stirs = Some(self.stirs.clone());
        self.volumes.clear();
        let mut q = world.query::<(Entity, &EntityId, &Voxels, Option<&Transform>)>();
        let mut rows: Vec<_> = q.iter(world).collect();
        rows.sort_unstable_by_key(|r| r.1.get());
        for (e, id, v, t) in rows {
            let mut sent = None;
            let origin = t.map_or([0.0; 3], |t| f3(t.position));
            if let Some(u) = volume_update(&mut sent, id.get(), v, origin) {
                frame.voxels.push(u);
            }
            if let Some(s) = sent {
                self.volumes.insert(e, s);
            }
        }
        self.waters.clear();
        let mut q = world.query::<(
            Entity,
            &EntityId,
            &WaterRegion,
            Option<&Transform>,
            Option<&WaterResident>,
        )>();
        let mut rows: Vec<_> = q.iter(world).collect();
        rows.sort_unstable_by_key(|r| r.1.get());
        for (e, id, r, t, state) in rows {
            let mut sent = None;
            let p = t.map_or([0.0; 3], |t| t.position);
            if let (Solver::GpuResident, Some(state)) = (r.solver, state) {
                frame.resident_water.push(resident_update(
                    &mut sent,
                    id.get(),
                    r,
                    state,
                    world.resource::<WaterCheckpoints>(),
                    [f(p[0]), f(p[2])],
                    tick,
                ));
            } else if let Some(u) = unstepped(world, &mut sent, id.get(), r, p, tick, None) {
                frame.water.push(u);
            }
            if let Some(s) = sent {
                self.waters.insert(e, s);
            }
        }
        self.terrains.clear();
        let mut q = world.query::<(Entity, &EntityId, &Terrain, Option<&Transform>)>();
        let mut rows: Vec<_> = q.iter(world).collect();
        rows.sort_unstable_by_key(|r| r.1.get());
        for (e, id, t, at) in rows {
            frame.terrain.push(terrain_view(
                id.get(),
                t,
                at.map_or([0.0; 3], |t| t.position),
            ));
            self.terrains.insert(e, id.get());
        }
        frame
    }

    fn delta(&mut self, world: &mut World) -> RenderFrame {
        let (tick, t_s, dt_s) = Self::clock(world);
        let mut frame = RenderFrame {
            tick,
            t_s,
            dt_s,
            ..RenderFrame::default()
        };
        // Despawned entities and removed models.
        let removed: Vec<Entity> = world
            .removed::<Model>()
            .chain(world.removed::<Transform>())
            .collect();
        for e in removed {
            let still = world
                .get_entity(e)
                .ok()
                .is_some_and(|r| r.contains::<Model>() && r.contains::<Transform>());
            if !still && let Some(id) = self.drawn.remove(&e) {
                frame.removed.push(id);
            }
        }
        if self.queries.as_ref().is_none_or(|q| q.world != world.id()) {
            self.queries = Some(Queries::new(world));
        }
        let Some(q) = self.queries.as_mut() else {
            return frame;
        };
        let w: &World = world;
        for (e, id, t, m, a) in q.instances.iter(w) {
            let new = !self.drawn.contains_key(&e);
            let look_changed = new || m.is_changed();
            let anim_changed = a.as_ref().is_some_and(|a| new || a.is_changed());
            if new || t.is_changed() || look_changed || anim_changed {
                if new {
                    self.drawn.insert(e, id.get());
                }
                frame.instances.push(InstanceUpdate {
                    id: id.get(),
                    pose: Some(pose(&t, m.scale)),
                    look: look_changed.then(|| look(&m)),
                    anim: if anim_changed {
                        a.map(|a| anim(&a))
                    } else {
                        None
                    },
                });
            }
        }
        let lights = q
            .lights
            .iter(w)
            .any(|(t, l)| t.is_changed() || l.is_changed())
            || w.removed::<Light>().next().is_some();
        let cameras = q
            .cameras
            .iter(w)
            .any(|(t, c)| t.is_changed() || c.is_changed())
            || w.removed::<Camera>().next().is_some();
        let environment = q.environment.iter(w).any(|e| e.is_changed());
        // The wind raises the wind sea drawn on the sea.
        let sea = q.sea.iter(w).any(|s| s.is_changed())
            || w.removed::<Sea>().next().is_some()
            || q.wind.iter(w).any(|s| s.is_changed())
            || w.removed::<Wind>().next().is_some();
        // The weather follows the wind, the rain and the regions they fall on.
        let weather = q.wind.iter(w).any(|s| s.is_changed())
            || w.removed::<Wind>().next().is_some()
            || q.rain.iter(w).any(|s| s.is_changed())
            || w.removed::<Rain>().next().is_some()
            || q.regions.iter(w).any(|r| r.is_changed())
            || w.removed::<WaterRegion>().next().is_some();
        let emitters = q
            .emitters
            .iter(w)
            .any(|(t, a)| a.is_changed() || t.is_some_and(|t| t.is_changed()))
            || w.removed::<ParticleEmitter>().next().is_some();
        let audio = q
            .audio
            .iter(w)
            .any(|(t, a)| a.is_changed() || t.is_some_and(|t| t.is_changed()))
            || w.removed::<AudioSource>().next().is_some();
        let ui = q.ui.iter(w).any(|(t, a, b)| {
            t.is_some_and(|t| t.is_changed())
                || a.is_some_and(|a| a.is_changed())
                || b.is_some_and(|b| b.is_changed())
        }) || w.removed::<UiText>().next().is_some()
            || w.removed::<UiBar>().next().is_some();
        let splats = q
            .splats
            .iter(w)
            .any(|(t, s)| t.is_changed() || s.is_changed())
            || w.removed::<Splat>().next().is_some();
        if lights {
            frame.lights = Some(Self::lights(world));
        }
        if cameras {
            frame.cameras = Some(Self::cameras(world));
        }
        if environment {
            frame.environment = Self::environment(world);
        }
        if sea {
            frame.sea = Some(Self::sea(world));
        }
        if weather {
            frame.weather = Some(Self::weather(world));
        }
        if emitters {
            frame.emitters = Some(Self::emitters(world));
        }
        if audio {
            frame.audio = Some(Self::audio(world));
        }
        if ui {
            frame.ui = Some(Self::ui(world));
        }
        if splats {
            frame.splats = Some(Self::splats(world));
        }
        // The bodies stirring the water move nearly every tick; compared whole, a still scene
        // sends none.
        let stirs = crate::stirs::stirs(world);
        if stirs != self.stirs {
            self.stirs = stirs;
            frame.stirs = Some(self.stirs.clone());
        }
        // Block volumes: the chunks that changed, and the volumes gone.
        let gone: Vec<Entity> = world.removed::<Voxels>().collect();
        for e in gone {
            let still = world
                .get_entity(e)
                .ok()
                .is_some_and(|r| r.contains::<Voxels>());
            if !still && let Some(s) = self.volumes.remove(&e) {
                frame.voxels_removed.push(s.id);
            }
        }
        if let Some(q) = self.queries.as_mut() {
            let mut rows: Vec<_> = q
                .voxels
                .iter(&*world)
                .filter(|(e, _, v, t)| {
                    !self.volumes.contains_key(e)
                        || v.is_changed()
                        || t.as_ref().is_some_and(|t| t.is_changed())
                })
                .map(|(e, id, v, t)| (e, id.get(), v, t))
                .collect();
            rows.sort_unstable_by_key(|r| r.1);
            for (e, id, v, t) in rows {
                let mut sent = self.volumes.remove(&e);
                let origin = t.map_or([0.0; 3], |t| f3(t.position));
                if let Some(u) = volume_update(&mut sent, id, &v, origin) {
                    frame.voxels.push(u);
                }
                if let Some(s) = sent {
                    self.volumes.insert(e, s);
                }
            }
        }
        // Terrains: whole when they changed, and those gone.
        let gone: Vec<Entity> = world.removed::<Terrain>().collect();
        for e in gone {
            let still = world
                .get_entity(e)
                .ok()
                .is_some_and(|r| r.contains::<Terrain>());
            if !still && let Some(id) = self.terrains.remove(&e) {
                frame.terrain_removed.push(id);
            }
        }
        let mut grounds = false;
        if let Some(q) = self.queries.as_mut() {
            let mut rows: Vec<_> = q
                .terrain
                .iter(&*world)
                .filter(|(e, _, t, at)| {
                    !self.terrains.contains_key(e)
                        || t.is_changed()
                        || at.as_ref().is_some_and(|t| t.is_changed())
                })
                .map(|(e, id, t, at)| (e, id.get(), t, at))
                .collect();
            rows.sort_unstable_by_key(|r| r.1);
            for (e, id, t, at) in rows {
                frame
                    .terrain
                    .push(terrain_view(id, &t, at.map_or([0.0; 3], |t| t.position)));
                self.terrains.insert(e, id);
                grounds = true;
            }
        }
        // Water regions: the tiles stepped since they were last sent, and the regions gone.
        let gone: Vec<Entity> = world.removed::<WaterRegion>().collect();
        for e in gone {
            let still = world
                .get_entity(e)
                .ok()
                .is_some_and(|r| r.contains::<WaterRegion>());
            if !still && let Some(s) = self.waters.remove(&e) {
                frame.water_removed.push(s.id);
            }
        }
        if let Some(q) = self.queries.as_mut() {
            let steps = world.get_resource::<WaterSteps>();
            let mut rows: Vec<_> = q
                .water
                .iter(world)
                .filter(|(e, _, r, t, state)| {
                    !self.waters.contains_key(e)
                        || r.is_changed()
                        || state.as_ref().is_some_and(|s| s.is_changed())
                        || t.as_ref().is_some_and(|t| t.is_changed())
                        // A preview follows its terrain.
                        || (grounds && state.is_none() && matches!(r.bed, BedSource::Terrain { .. }))
                })
                .map(|(e, id, r, t, state)| (e, *id, r, t, state))
                .collect();
            rows.sort_unstable_by_key(|r| r.1);
            for (e, id, r, t, state) in rows {
                let mut sent = self.waters.remove(&e);
                let p = t.map_or([0.0; 3], |t| t.position);
                let marks = steps.and_then(|s| s.regions.get(&id));
                if let (Solver::GpuResident, Some(state)) = (r.solver, state.as_ref()) {
                    // Shown as its tiles until its first step: its field replaces them whole.
                    if sent.as_ref().is_some_and(|s| s.checkpoint.is_none()) {
                        sent = None;
                    }
                    frame.resident_water.push(resident_update(
                        &mut sent,
                        id.get(),
                        &r,
                        state,
                        world.resource::<WaterCheckpoints>(),
                        [f(p[0]), f(p[2])],
                        tick,
                    ));
                } else if let Some(u) = unstepped(world, &mut sent, id.get(), &r, p, tick, marks) {
                    frame.water.push(u);
                }
                if let Some(s) = sent {
                    self.waters.insert(e, s);
                }
            }
        }
        frame
    }

    fn lights(world: &mut World) -> Vec<LightView> {
        let mut v: Vec<LightView> = world
            .query::<(&EntityId, &Transform, &Light)>()
            .iter(world)
            .map(|(id, t, l)| LightView {
                id: id.get(),
                kind: match l.kind {
                    LightKind::Directional => LightKindView::Directional,
                    LightKind::Point => LightKindView::Point,
                    LightKind::Spot => LightKindView::Spot,
                },
                position: f3(t.position),
                direction: forward(t),
                color: f3(l.color),
                intensity: f(l.intensity),
                range: f(l.range),
                inner_deg: f(l.inner_deg),
                outer_deg: f(l.outer_deg),
                shadows: l.shadows,
            })
            .collect();
        v.sort_by_key(|l| l.id);
        v
    }

    fn cameras(world: &mut World) -> Vec<CameraView> {
        let mut v: Vec<CameraView> = world
            .query::<(&EntityId, &Transform, &Camera)>()
            .iter(world)
            .map(|(id, t, c)| CameraView {
                id: id.get(),
                position: f3(t.position),
                rotation: [
                    f(t.rotation[0]),
                    f(t.rotation[1]),
                    f(t.rotation[2]),
                    f(t.rotation[3]),
                ],
                fov_deg: f(c.fov_deg),
                near: f(c.near),
                far: f(c.far),
                active: c.active,
                exposure_ev: f(c.exposure_ev),
            })
            .collect();
        v.sort_by_key(|c| c.id);
        v
    }

    fn environment(world: &mut World) -> Option<EnvironmentView> {
        world
            .query::<(&EntityId, &Environment)>()
            .iter(world)
            .min_by_key(|(id, _)| id.get())
            .map(|(_, e)| env_view(e))
    }

    fn sea(world: &mut World) -> Option<SeaView> {
        // The wind sea grows from the wind's mean speed and direction over the sea's fetch.
        let wind = world
            .query::<(&EntityId, &Wind)>()
            .iter(world)
            .min_by_key(|(id, _)| id.get())
            .map(|(_, w)| *w);
        world
            .query::<(&EntityId, &Sea)>()
            .iter(world)
            .min_by_key(|(id, _)| id.get())
            .map(|(_, s)| SeaView {
                level: f(s.level),
                waves: s
                    .waves
                    .iter()
                    .map(|w| WaveView {
                        length: f(w.length),
                        height: f(w.height),
                        toward_deg: f(w.toward_deg),
                        phase: f(w.phase),
                    })
                    .collect(),
                wind: wind
                    .filter(|w| s.fetch_km > 0.0 && w.speed > 0.0)
                    .map(|w| WindSeaView {
                        speed: f(w.speed),
                        toward_deg: f((w.from_deg + 180.0).rem_euclid(360.0)),
                        fetch_km: f(s.fetch_km),
                    }),
                look: s.look.map(|l| SeaLookView {
                    fog: l.fog,
                    absorb: l.absorb,
                    scatter: l.scatter,
                }),
            })
    }

    /// The weather on the water (water.md 67): the first `Wind`'s sea over the regions it blows
    /// on, its fetch their longest reach along it, and the first `Rain`.
    fn weather(world: &mut World) -> WeatherView {
        let wind = world
            .query::<(&EntityId, &Wind)>()
            .iter(world)
            .min_by_key(|(id, _)| id.get())
            .map(|(_, w)| *w);
        let rain = world
            .query::<(&EntityId, &Rain)>()
            .iter(world)
            .min_by_key(|(id, _)| id.get())
            .map(|(_, r)| *r);
        let mut reach = |toward: amoris_physics::geom::V3| {
            world
                .query::<&WaterRegion>()
                .iter(world)
                .filter(|r| r.physics.wind)
                .map(|r| {
                    toward[0].abs() * f64::from(r.size[0]) * r.cell_m
                        + toward[2].abs() * f64::from(r.size[1]) * r.cell_m
                })
                .fold(0.0, f64::max)
        };
        WeatherView {
            wind: wind.filter(|w| w.speed > 0.0).and_then(|w| {
                let fetch = reach(amoris_physics::geom::from_bearing(w.from_deg + 180.0));
                (fetch > 0.0).then(|| WindSeaView {
                    speed: f(w.speed),
                    toward_deg: f((w.from_deg + 180.0).rem_euclid(360.0)),
                    fetch_km: f(fetch / 1000.0),
                })
            }),
            rain: rain.filter(|r| r.intensity_mm_h > 0.0).map(|r| RainView {
                intensity_mm_h: f(r.intensity_mm_h),
                drop_mm: f(r.drop_mm),
            }),
        }
    }

    fn splats(world: &mut World) -> Vec<SplatView> {
        let mut v: Vec<SplatView> = world
            .query::<(&EntityId, &Transform, &Splat)>()
            .iter(world)
            .map(|(id, t, s)| SplatView {
                id: id.get(),
                asset: s.asset.clone(),
                pose: pose(t, [s.scale; 3]),
                visible: s.visible,
            })
            .collect();
        v.sort_by_key(|s| s.id);
        v
    }
}

fn anchor_code(a: UiAnchor) -> u32 {
    match a {
        UiAnchor::TopLeft => 0,
        UiAnchor::Top => 1,
        UiAnchor::TopRight => 2,
        UiAnchor::Left => 3,
        UiAnchor::Center => 4,
        UiAnchor::Right => 5,
        UiAnchor::BottomLeft => 6,
        UiAnchor::Bottom => 7,
        UiAnchor::BottomRight => 8,
        UiAnchor::Entity => 9,
    }
}

impl Extractor {
    fn emitters(world: &mut World) -> Vec<EmitterView> {
        let mut v: Vec<EmitterView> = world
            .query::<(&EntityId, Option<&Transform>, &ParticleEmitter)>()
            .iter(world)
            .map(|(id, t, e)| {
                let up = t.map_or([0.0, 1.0, 0.0], |t| {
                    let q = t.rotation;
                    // The entity's +y: the rotation applied to (0, 1, 0).
                    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
                    f3([
                        2.0 * (x * y - w * z),
                        1.0 - 2.0 * (x * x + z * z),
                        2.0 * (y * z + w * x),
                    ])
                });
                EmitterView {
                    id: id.get(),
                    position: t.map_or([0.0; 3], |t| f3(t.position)),
                    up,
                    rate: f(e.rate),
                    burst: e.burst,
                    burst_id: e.burst_id,
                    emitting: e.emitting,
                    lifetime: f(e.lifetime),
                    speed: f(e.speed),
                    spread_deg: f(e.spread_deg),
                    acceleration: f3(e.acceleration),
                    drag: f(e.drag),
                    size: [f(e.size[0]), f(e.size[1])],
                    color_start: [
                        f(e.color_start[0]),
                        f(e.color_start[1]),
                        f(e.color_start[2]),
                        f(e.color_start[3]),
                    ],
                    color_end: [
                        f(e.color_end[0]),
                        f(e.color_end[1]),
                        f(e.color_end[2]),
                        f(e.color_end[3]),
                    ],
                    radius: f(e.radius),
                }
            })
            .collect();
        v.sort_by_key(|e| e.id);
        v
    }

    fn audio(world: &mut World) -> Vec<AudioView> {
        let mut v: Vec<AudioView> = world
            .query::<(&EntityId, Option<&Transform>, &AudioSource)>()
            .iter(world)
            .map(|(id, t, a)| AudioView {
                id: id.get(),
                clip: a.clip.clone(),
                volume: f(a.volume),
                pitch: f(a.pitch),
                looped: a.looped,
                playing: a.playing,
                spatial: a.spatial,
                position: t.map_or([0.0; 3], |t| f3(t.position)),
            })
            .collect();
        v.sort_by_key(|a| a.id);
        v
    }

    fn ui(world: &mut World) -> Vec<UiView> {
        let mut v: Vec<UiView> = Vec::new();
        let mut q = world.query::<(
            &EntityId,
            Option<&Transform>,
            Option<&UiText>,
            Option<&UiBar>,
        )>();
        for (id, t, text, bar) in q.iter(world) {
            let base = t.map_or([0.0; 3], |t| f3(t.position));
            let at = |o: [f64; 3]| [base[0] + f(o[0]), base[1] + f(o[1]), base[2] + f(o[2])];
            if let Some(b) = bar.filter(|b| b.visible) {
                let fill = if b.max > 0.0 {
                    (b.value / b.max).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                v.push(UiView {
                    id: id.get(),
                    text: String::new(),
                    fill: f(fill),
                    bar: true,
                    size: [f(b.size[0]), f(b.size[1])],
                    color: [f(b.color[0]), f(b.color[1]), f(b.color[2]), f(b.color[3])],
                    back: [f(b.back[0]), f(b.back[1]), f(b.back[2]), f(b.back[3])],
                    anchor: anchor_code(b.anchor),
                    offset: [f(b.offset[0]), f(b.offset[1])],
                    world: at(b.world_offset),
                });
            }
            if let Some(x) = text.filter(|x| x.visible && !x.text.is_empty()) {
                v.push(UiView {
                    id: id.get(),
                    text: x.text.clone(),
                    fill: 0.0,
                    bar: false,
                    size: [f(x.size), f(x.size)],
                    color: [f(x.color[0]), f(x.color[1]), f(x.color[2]), f(x.color[3])],
                    back: [0.0; 4],
                    anchor: anchor_code(x.anchor),
                    offset: [f(x.offset[0]), f(x.offset[1])],
                    world: at(x.world_offset),
                });
            }
        }
        v.sort_by_key(|u| (u.id, u.bar));
        v
    }
}

fn env_view(e: &Environment) -> EnvironmentView {
    EnvironmentView {
        sky: match e.sky {
            SkyKind::Atmosphere => 0,
            SkyKind::Color => 1,
        },
        sky_color: f3(e.sky_color),
        ambient: f(e.ambient),
        baked_gi: e.baked_gi.clone(),
        neural_gi: e.neural_gi.clone(),
        gi_intensity: f(e.gi_intensity),
        fog_density: f(e.fog_density),
        fog_color: f3(e.fog_color),
        exposure_ev: f(e.exposure_ev),
        bloom: f(e.bloom),
    }
}
