//! The render feed's producer (charter 4.4; pocket-assets `frame`): after a tick or a boundary
//! that wrote, the game thread extracts what changed in the visual state into a `RenderFrame` and
//! hands it to the feed's subscribers. Presentation only: change detection is read here and
//! nowhere in the simulation (pocket-sim's rule), and nothing here writes a component.
//!
//! A frame carries the instances whose `Transform` or `Model` changed since the last extraction,
//! the entities that lost their `Model` or were despawned, and the full lists of lights, cameras,
//! splats, the environment and the sea when any of them changed. A new subscriber, a restore, a
//! fork swapped in (Play, Stop) or a reload gets a full frame.

use std::collections::HashMap;

use bevy_ecs::change_detection::DetectChanges;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::World;
use bevy_ecs::world::{Ref, WorldId};
use pocket_assets::frame::{
    CameraView, EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
    SeaView, SplatView, WaveView,
};
use pocket_assets::{Camera, Environment, Feed, Light, LightKind, Model, SkyKind, Splat};
use pocket_physics::{Sea, Transform};
use pocket_sim::{EntityId, SimClock};

/// Extracts render frames from one game's world.
#[derive(Default)]
pub struct Extractor {
    /// The world and generation the last frame described; another one means a full frame.
    seen: Option<(WorldId, u64)>,
    /// Entities currently drawn, so a despawn (whose id is gone with the entity) can be reported.
    drawn: HashMap<Entity, u64>,
}

fn f(x: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        x as f32
    }
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

fn look(m: &Model) -> Look {
    Look {
        mesh: m.mesh.clone(),
        material: m.material.clone(),
        color: [f(m.color[0]), f(m.color[1]), f(m.color[2]), f(m.color[3])],
        metallic: f(m.metallic),
        roughness: f(m.roughness),
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
            // Keep the trackers fresh so the first subscriber does not see a stale backlog.
            self.seen = None;
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
        let mut q = world.query::<(Entity, &EntityId, &Transform, &Model)>();
        let mut rows: Vec<(u64, Entity, InstanceUpdate)> = q
            .iter(world)
            .map(|(e, id, t, m)| {
                (
                    id.get(),
                    e,
                    InstanceUpdate {
                        id: id.get(),
                        pose: Some(pose(t, m.scale)),
                        look: Some(look(m)),
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
        frame.splats = Some(Self::splats(world));
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
        let mut q = world.query::<(Entity, &EntityId, Ref<Transform>, Ref<Model>)>();
        for (e, id, t, m) in q.iter(world) {
            let new = !self.drawn.contains_key(&e);
            let look_changed = new || m.is_changed();
            if new || t.is_changed() || look_changed {
                if new {
                    self.drawn.insert(e, id.get());
                }
                frame.instances.push(InstanceUpdate {
                    id: id.get(),
                    pose: Some(pose(&t, m.scale)),
                    look: look_changed.then(|| look(&m)),
                });
            }
        }
        let changed = |world: &mut World, any: &mut dyn FnMut(&mut World) -> bool| any(world);
        if changed(world, &mut |w| {
            w.query::<(Ref<Transform>, Ref<Light>)>()
                .iter(w)
                .any(|(t, l)| t.is_changed() || l.is_changed())
                || w.removed::<Light>().next().is_some()
        }) {
            frame.lights = Some(Self::lights(world));
        }
        if changed(world, &mut |w| {
            w.query::<(Ref<Transform>, Ref<Camera>)>()
                .iter(w)
                .any(|(t, c)| t.is_changed() || c.is_changed())
                || w.removed::<Camera>().next().is_some()
        }) {
            frame.cameras = Some(Self::cameras(world));
        }
        if changed(world, &mut |w| {
            w.query::<Ref<Environment>>()
                .iter(w)
                .any(|e| e.is_changed())
        }) {
            frame.environment = Self::environment(world);
        }
        if changed(world, &mut |w| {
            w.query::<Ref<Sea>>().iter(w).any(|s| s.is_changed())
                || w.removed::<Sea>().next().is_some()
        }) {
            frame.sea = Some(Self::sea(world));
        }
        if changed(world, &mut |w| {
            w.query::<(Ref<Transform>, Ref<Splat>)>()
                .iter(w)
                .any(|(t, s)| t.is_changed() || s.is_changed())
                || w.removed::<Splat>().next().is_some()
        }) {
            frame.splats = Some(Self::splats(world));
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
            })
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

fn env_view(e: &Environment) -> EnvironmentView {
    EnvironmentView {
        sky: match e.sky {
            SkyKind::Atmosphere => 0,
            SkyKind::Color => 1,
        },
        sky_color: f3(e.sky_color),
        ambient: f(e.ambient),
        fog_density: f(e.fog_density),
        fog_color: f3(e.fog_color),
        exposure_ev: f(e.exposure_ev),
        bloom: f(e.bloom),
    }
}
