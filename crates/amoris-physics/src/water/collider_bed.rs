//! Current fixed geometry, not the previous Rapier broad phase. Sampled beds remain world state.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
use super::{BedSource, WaterRegion, WaterTile, resident::WaterResident};
use crate::{
    body::{BodyKind, Collider, RigidBody, Transform},
    geom, shapes,
};
use amoris_contract::{Problem, detail};
use amoris_sim::{ContentHash, EntityId, EntityIndex, NewEvent, PlainData, math};
use bevy_ecs::prelude::{Component, DetectChanges, World};
use rapier3d::math::Pose;
use rapier3d::prelude::{Ray, SharedShape};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Last geometry actually used for a bed. Required across restoration and late component edits.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WaterColliderBed {
    pub source: ContentHash,
}
impl amoris_sim::Persisted for WaterColliderBed {
    const NAME: &'static str = "WaterColliderBed";
    const VERSION: u32 = 1;
}
struct Entry {
    ticks: [u32; 3],
    shape: SharedShape,
    key: [u8; 32],
    pose: Pose,
    pose_values: [f64; 7],
    bounds: [[f64; 3]; 2],
}
#[derive(Default)]
pub(crate) struct Cache {
    entries: BTreeMap<EntityId, Entry>,
    pub events: Vec<NewEvent>,
}
fn failed(message: impl Into<String>) -> Problem {
    Problem::new("water.collider_bed", message.into(), detail([]))
}
impl Cache {
    fn synchronize(
        &mut self,
        world: &World,
        ids: &[(EntityId, bevy_ecs::entity::Entity)],
    ) -> Result<(), Problem> {
        let mut live = BTreeSet::new();
        for &(id, e) in ids {
            let Ok(entity) = world.get_entity(e) else {
                continue;
            };
            let Some(c) = entity.get_ref::<Collider>() else {
                continue;
            };
            let body = entity.get_ref::<RigidBody>();
            if body.as_ref().is_some_and(|b| b.kind != BodyKind::Fixed) {
                continue;
            }
            let tf = entity
                .get_ref::<Transform>()
                .ok_or_else(|| failed(format!("fixed collider {id} needs Transform")))?;
            let ticks = [
                c.last_changed().get(),
                tf.last_changed().get(),
                body.as_ref().map_or(0, |b| b.last_changed().get()),
            ];
            live.insert(id);
            if self.entries.get(&id).is_some_and(|old| old.ticks == ticks) {
                continue;
            }
            if tf
                .position
                .iter()
                .any(|v| !v.is_finite() || v.abs() > geom::SOLVER_MAX)
            {
                return Err(failed(format!(
                    "collider {id} position exceeds the solver range"
                )));
            }
            let rotation = geom::normalized(tf.rotation)
                .ok_or_else(|| failed(format!("collider {id} has an invalid rotation")))?;
            let pose = geom::pose_in(tf.position, rotation);
            let p = geom::vec_out(pose.translation);
            let q = geom::quat_out(pose.rotation);
            let pose_values = [p[0], p[1], p[2], q[0], q[1], q[2], q[3]];
            let unchanged_shape = self
                .entries
                .get(&id)
                .is_some_and(|old| old.ticks[0] == ticks[0] && old.ticks[2] == ticks[2]);
            if unchanged_shape
                && self.entries.get(&id).is_some_and(|old| {
                    old.pose_values.map(f64::to_bits) == pose_values.map(f64::to_bits)
                })
            {
                self.entries.get_mut(&id).unwrap().ticks = ticks;
                continue;
            }
            let (shape, key) = if unchanged_shape {
                let old = self.entries.get(&id).unwrap();
                (old.shape.clone(), old.key)
            } else {
                crate::body::validate(id, body.as_deref(), &c)?;
                let (spec, build_key) = shapes::spec_and_key(body.as_deref(), &c);
                let mut h = ContentHash::hasher("Amoris collider water bed shape v1");
                h.update(
                    &bincode::serialize(&c.shape)
                        .map_err(|_| failed("collider shape cannot be encoded"))?,
                );
                (shapes::fixed(id, &c, spec, build_key)?, h.finish().0)
            };
            let aabb = shape.compute_aabb(&pose);
            let bounds = [geom::vec_out(aabb.mins), geom::vec_out(aabb.maxs)];
            self.entries.insert(
                id,
                Entry {
                    ticks,
                    shape,
                    key,
                    pose,
                    pose_values,
                    bounds,
                },
            );
        }
        self.entries.retain(|id, _| live.contains(id));
        Ok(())
    }
    fn selected<'a>(
        &'a self,
        r: &WaterRegion,
        origin: [f64; 2],
        top: f64,
        bottom: f64,
    ) -> impl Iterator<Item = (&'a EntityId, &'a Entry)> {
        let begin = [origin[0] + 0.5 * r.cell_m, origin[1] + 0.5 * r.cell_m];
        let end = [
            origin[0] + (f64::from(r.size[0]) - 0.5) * r.cell_m,
            origin[1] + (f64::from(r.size[1]) - 0.5) * r.cell_m,
        ];
        self.entries.iter().filter(move |(_, e)| {
            let pad = e
                .bounds
                .iter()
                .flatten()
                .map(|v| v.abs())
                .fold(1.0f64, math::max)
                * f64::from(f32::EPSILON)
                * 2.0;
            e.bounds[0][0] - pad <= end[0]
                && e.bounds[1][0] + pad >= begin[0]
                && e.bounds[0][2] - pad <= end[1]
                && e.bounds[1][2] + pad >= begin[1]
                && e.bounds[0][1] - pad <= top
                && e.bounds[1][1] + pad >= bottom
        })
    }
    fn key(&self, r: &WaterRegion, origin: [f64; 2], top: f64, bottom: f64) -> ContentHash {
        let mut h = ContentHash::hasher(
            "Amoris collider water bed inputs v1 parry3d 0.31.1 enhanced-determinism",
        );
        for n in r.size {
            h.update(&n.to_le_bytes());
        }
        for v in [origin[0], origin[1], r.cell_m, top, bottom] {
            h.update(&v.to_bits().to_le_bytes());
        }
        for (id, e) in self.selected(r, origin, top, bottom) {
            h.update(&id.get().to_le_bytes());
            h.update(&e.key);
            for v in e.pose_values {
                h.update(&v.to_bits().to_le_bytes());
            }
        }
        h.finish()
    }
    fn heights(
        &self,
        r: &WaterRegion,
        origin: [f64; 2],
        top: f64,
        bottom: f64,
    ) -> Result<Vec<f64>, Problem> {
        let [nx, nz] = r.size.map(|n| n as usize);
        let mut bed = vec![bottom; nx * nz];
        let end = [
            origin[0] + nx as f64 * r.cell_m,
            origin[1] + nz as f64 * r.cell_m,
        ];
        if [origin[0], origin[1], end[0], end[1]]
            .iter()
            .any(|v| !v.is_finite() || v.abs() > geom::SOLVER_MAX)
        {
            return Err(failed("collider-bed grid exceeds the solver range"));
        }
        for (_, e) in self.selected(r, origin, top, bottom) {
            let pad = e
                .bounds
                .iter()
                .flatten()
                .map(|v| v.abs())
                .fold(1.0f64, math::max)
                * f64::from(f32::EPSILON)
                * 2.0;
            let mut lo = [0; 2];
            let mut hi = [0; 2];
            for axis in 0..2 {
                let world = axis * 2;
                lo[axis] = (math::max(
                    ((e.bounds[0][world] - pad - origin[axis]) / r.cell_m - 0.5).ceil(),
                    0.0,
                ) as usize)
                    .min(r.size[axis] as usize);
                hi[axis] = math::min(
                    math::max(
                        math::max(
                            ((e.bounds[1][world] + pad - origin[axis]) / r.cell_m - 0.5).floor(),
                            -1.0,
                        ) + 1.0,
                        0.0,
                    ),
                    f64::from(r.size[axis]),
                ) as usize;
            }
            for z in lo[1]..hi[1] {
                for x in lo[0]..hi[0] {
                    let ray = Ray::new(
                        geom::vec_in([
                            origin[0] + (x as f64 + 0.5) * r.cell_m,
                            top,
                            origin[1] + (z as f64 + 0.5) * r.cell_m,
                        ]),
                        geom::vec_in([0.0, -1.0, 0.0]),
                    );
                    if let Some(t) = e.shape.cast_ray(
                        &e.pose,
                        &ray,
                        math::max_f32(geom::f32_of(top) - geom::f32_of(bottom), 0.0),
                        true,
                    ) {
                        let y = f64::from(ray.point_at(t).y).clamp(bottom, top);
                        bed[x + nx * z] = math::max(bed[x + nx * z], y);
                    }
                }
            }
        }
        Ok(bed)
    }
}

/// Runs before physics.water borrows its normal system parameters. Never repeated on resume.
pub(crate) fn prepare(world: &mut World, cache: &mut Cache) -> Result<(), Problem> {
    let mut ids = world
        .get_resource::<EntityIndex>()
        .map_or_else(Vec::new, |i| i.iter().collect::<Vec<_>>());
    ids.sort_by_key(|(id, _)| *id);
    for &(_, e) in &ids {
        if world.get::<WaterColliderBed>(e).is_some()
            && !world
                .get::<WaterRegion>(e)
                .is_some_and(|r| matches!(r.bed, BedSource::Colliders { .. }))
        {
            if world.get::<WaterRegion>(e).is_some() && world.get::<WaterResident>(e).is_some() {
                return Err(super::resident::failed(
                    "resident collider-bed source changed; reset/materialize the field first",
                ));
            }
            world.entity_mut(e).remove::<WaterColliderBed>();
        }
    }
    let regions = ids
        .iter()
        .filter_map(|&(id, e)| {
            let r = world.get::<WaterRegion>(e)?;
            matches!(r.bed, BedSource::Colliders { .. }).then_some((id, e))
        })
        .collect::<Vec<_>>();
    if regions.is_empty() {
        cache.entries.clear();
        return Ok(());
    }
    cache.synchronize(world, &ids)?;
    for (id, e) in regions {
        let r = world.get::<WaterRegion>(e).unwrap();
        let BedSource::Colliders { top, bottom } = r.bed else {
            unreachable!()
        };
        let p = world.get::<Transform>(e).map_or([0.0; 3], |t| t.position);
        let origin = [p[0], p[2]];
        let key = cache.key(r, origin, top, bottom);
        let same = world
            .get::<WaterColliderBed>(e)
            .is_some_and(|s| s.source == key);
        let resident = world.get::<WaterResident>(e).is_some();
        if resident {
            if !same {
                return Err(super::resident::failed(
                    "resident collider-bed inputs changed; reset/materialize the field before rebuilding its bed",
                ));
            }
            continue;
        }
        if same && r.well_formed() {
            continue;
        }
        r.validate()?;
        let heights = cache.heights(r, origin, top, bottom)?;
        {
            let mut r = world.get_mut::<WaterRegion>(e).unwrap();
            let fresh = r.tiles.is_empty();
            if fresh {
                let [tx, tz] = r.size.map(|n| n / 16);
                r.tiles = (0..tx * tz)
                    .map(|i| WaterTile::dry([i % tx, i / tx], bottom))
                    .collect();
            }
            if !r.well_formed() {
                return Err(failed(
                    "collider bed needs a complete or empty initial grid",
                ));
            }
            let nx = r.size[0] as usize;
            let changes = heights
                .into_iter()
                .enumerate()
                .filter_map(|(i, h)| {
                    let c = [i % nx, i / nx];
                    let (t, n) = r.locate(c[0], c[1]);
                    (r.tiles[t].bed[n] != h).then_some((c, h))
                })
                .collect::<Vec<_>>();
            let displaced = super::bed::apply(&mut r, &changes);
            if fresh {
                super::author::fill(&mut r, origin);
            }
            for d in displaced {
                if let Some(event) = super::event(
                    "water.displaced",
                    id,
                    vec![
                        (
                            "column",
                            PlainData::Array(
                                d.column.map(|n| PlainData::Number(n as f64)).to_vec(),
                            ),
                        ),
                        ("m3", PlainData::Number(d.m3)),
                    ],
                ) {
                    cache.events.push(event);
                }
            }
        }
        world.entity_mut(e).insert(WaterColliderBed { source: key });
    }
    Ok(())
}
