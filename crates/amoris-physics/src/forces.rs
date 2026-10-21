//! The showcase's forces as engine systems (charter 2.4.1, 4.3; simulation.md 4.4 rows 4 to 7):
//! `physics.wind`, `physics.buoyancy`, `physics.sail` and `physics.hull`. Each reads the state at
//! the tick's start (the components `physics.step` wrote back, and any write since), computes in
//! `f64` with `amoris_sim::math`, and adds to each body's `ExternalForce`, which `physics.step`
//! hands the solver. Bodies are visited in `EntityId` order. The sea and the wind are evaluated at
//! the tick's start, `(n - 1) / rate` for tick n.

use amoris_sim::math::{self, clamp};
use amoris_sim::order::{by_id, by_id_mut};
use amoris_sim::{EntityId, SimClock, Tick};
use bevy_ecs::prelude::{Query, Res};

use crate::boat::{
    BOOM_MAX_DEG, Boat, Floater, HOIST_RATE, Hull, LUFF_DEG, NO_GO_HALF_DEG, NO_GO_RAMP_DEG,
    RUDDER_RATE, SHEET_RATE, STALL_DEG, Sail, Trim,
};
use crate::body::{ExternalForce, RigidBody, Transform, Velocity, local_com};
use crate::geom::{self, V3};
use crate::sea::{GRAVITY, Sea, WaveField, Wind};
use crate::solver::velocity_at;

/// The simulated time at the start of the running tick.
pub fn start_time(clock: &SimClock) -> f64 {
    Tick(clock.tick.0.saturating_sub(1)).to_f64() / f64::from(clock.rate.0)
}

/// `now` moved toward `target` by at most `step`.
fn follow(now: f64, target: f64, step: f64) -> f64 {
    if target > now {
        math::min(now + step, target)
    } else {
        math::max(now - step, target)
    }
}

fn smoothstep(x: f64) -> f64 {
    let t = clamp(x, 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `x |x|`: the signed square of drag and plate pressure.
fn signed_sq(x: f64) -> f64 {
    x * x.abs()
}

/// The first sea's field, by id.
fn sea_field(seas: &Query<(&EntityId, &Sea)>) -> Option<WaveField> {
    by_id(seas).next().map(|(_, s)| s.field())
}

/// `physics.wind`: the true wind at each sail's mast (the first `Wind` by id; none is calm).
pub(crate) fn wind(
    clock: Res<SimClock>,
    winds: Query<(&EntityId, &Wind)>,
    mut sails: Query<(&EntityId, &Transform, &mut Sail)>,
) {
    let t = start_time(&clock);
    let w = by_id(&winds).next().map(|(_, w)| *w);
    for (_, tf, mut sail) in by_id_mut(&mut sails) {
        let mast = geom::to_world(tf.position, tf.rotation, sail.mast);
        sail.wind = w.map_or(geom::ZERO, |w| w.at(mast, t));
    }
}

type FloaterItem<'a> = (
    &'a EntityId,
    &'a Transform,
    Option<&'a Velocity>,
    Option<&'a RigidBody>,
    Option<&'a crate::body::Collider>,
    &'a mut Floater,
    Option<&'a mut ExternalForce>,
);

/// `physics.buoyancy`: Archimedes at each buoyancy point under the surface (the point displaces
/// its volume times how far under it is, ramped over its layer) and drag toward the water's own
/// velocity there; `Floater::submerged` is written for every floater.
pub(crate) fn buoyancy(
    clock: Res<SimClock>,
    seas: Query<(&EntityId, &Sea)>,
    regions: Query<(&EntityId, Option<&Transform>, &crate::water::WaterRegion)>,
    samples: Res<crate::water::buoyancy::Samples>,
    mut feedbacks: Query<(
        bevy_ecs::entity::Entity,
        &EntityId,
        &mut crate::water::feedback::WaterFeedback,
    )>,
    mut floaters: Query<FloaterItem<'static>>,
) -> Result<(), amoris_contract::Problem> {
    if floaters.is_empty() {
        return Ok(());
    }
    let t = start_time(&clock);
    let field = sea_field(&seas);
    let feedback_index: std::collections::BTreeMap<_, _> =
        by_id(&feedbacks).map(|(e, id, _)| (*id, e)).collect();
    let regions: Vec<_> = by_id(&regions)
        .filter_map(|(id, t, r)| {
            crate::water::buoyancy::Cover::new(*id, t.map_or([0.0; 3], |t| t.position), r)
                .map(|cover| (cover, r))
        })
        .collect();
    for (id, tf, vel, body, collider, mut f, ext) in by_id_mut(&mut floaters) {
        let v = vel.copied().unwrap_or_default();
        let com = geom::to_world(tf.position, tf.rotation, local_com(body));
        let mut gathered = ExternalForce::default();
        let (mut wet, mut total) = (0.0, 0.0);
        for (point, p) in f.points.iter().enumerate() {
            total += p.volume;
            let at = geom::to_world(tf.position, tf.rotation, p.at);
            let region = regions
                .iter()
                .find_map(|(cover, r)| cover.grid_at(at).map(|grid| (cover, grid, *r)));
            let reaction = region
                .as_ref()
                .and_then(|(cover, grid, _)| feedback_index.get(&cover.id).map(|e| (*e, *grid)));
            let (water, density, bed) = if let Some((cover, grid, region)) = region {
                let column = if region.solver == crate::water::Solver::GpuResident {
                    samples
                        .get((*id, point), at, cover.id, clock.tick.0)
                        .ok_or_else(|| {
                            crate::water::resident::failed(
                                "resident buoyancy has no current-pose sample",
                            )
                        })?
                } else {
                    crate::water::buoyancy::sample(region, grid)
                };
                if column[1] <= 0.0 {
                    continue;
                }
                (
                    crate::sea::Water {
                        height: column[0],
                        velocity: [column[2], 0.0, column[3]],
                    },
                    1000.0,
                    Some(column[0] - column[1]),
                )
            } else if let Some(field) = &field {
                (field.at(at[0], at[2], t), field.density, None)
            } else {
                continue;
            };
            let depth = water.height - at[1];
            let mut under = if p.size > 0.0 {
                clamp(depth / p.size + 0.5, 0.0, 1.0)
            } else if depth > 0.0 {
                1.0
            } else {
                0.0
            };
            // Only the part of a buoyancy layer between bed and surface displaces field water.
            if let Some(bed) = bed {
                under -= if p.size > 0.0 {
                    clamp((bed - at[1]) / p.size + 0.5, 0.0, 1.0)
                } else if bed > at[1] {
                    1.0
                } else {
                    0.0
                };
            }
            if under <= 0.0 {
                continue;
            }
            let displaced = p.volume * under;
            wet += displaced;
            let k = density * displaced;
            let rel = geom::sub(water.velocity, velocity_at(&v, com, at));
            let force = [
                rel[0] * f.drag * k,
                density * GRAVITY * displaced + rel[1] * f.heave * k,
                rel[2] * f.drag * k,
            ];
            gathered.add_at(force, at, com);
            if ext.is_some()
                && collider.is_some()
                && let Some((e, grid)) = reaction
                && let Ok((_, _, mut target)) = feedbacks.get_mut(e)
            {
                target.queue(
                    clock.tick.0,
                    grid,
                    [-force[0] * clock.dt(), -force[2] * clock.dt()],
                )?;
            }
        }
        f.submerged = if total > 0.0 { wet / total } else { 0.0 };
        if let Some(mut e) = ext {
            e.force = geom::add(e.force, gathered.force);
            e.torque = geom::add(e.torque, gathered.torque);
        }
    }
    Ok(())
}

type SailItem<'a> = (
    &'a EntityId,
    &'a Transform,
    Option<&'a Velocity>,
    Option<&'a RigidBody>,
    &'a mut Boat,
    &'a Sail,
    Option<&'a mut ExternalForce>,
);

/// The forward drive of a flat sail at boom angle `b` in a wind streaming at `free` (both from the
/// centreline, `b <= free`): `sin^2(free - b) sin b`.
fn drive_at(free: f64, b: f64) -> f64 {
    let s = math::sin(free - b);
    s * s * math::sin(b)
}

/// The drive at `boom` as a share of the best boom angle's, sampled over the boom's range.
fn drive_share(free: f64, boom: f64) -> f64 {
    let reach = math::min(free, BOOM_MAX_DEG.to_radians());
    let mut best = 0.0;
    for k in 0..=32 {
        best = math::max(best, drive_at(free, reach * f64::from(k) / 32.0));
    }
    if best > 0.0 {
        clamp(drive_at(free, boom) / best, 0.0, 1.0)
    } else {
        0.0
    }
}

/// `physics.sail`: the hoist and the sheet follow their controls; the sail, a flat plate whose boom
/// swings to leeward as far as the sheet lets it and streams with the apparent wind within it
/// (luffing), takes `coef (v . n) |v . n|` along its normal, nothing within `NO_GO_HALF_DEG` of the
/// apparent wind's eye; the apparent wind, boom, drive and trim readings are written.
pub(crate) fn sail(clock: Res<SimClock>, mut boats: Query<SailItem<'static>>) {
    let dt = clock.dt();
    for (_, tf, vel, body, mut boat, sail, ext) in by_id_mut(&mut boats) {
        boat.hoist_now = follow(boat.hoist_now, clamp(boat.hoist, 0.0, 1.0), HOIST_RATE * dt);
        boat.sheet_now = follow(boat.sheet_now, clamp(boat.sheet, 0.0, 1.0), SHEET_RATE * dt);
        let v = vel.copied().unwrap_or_default();
        let (pos, rot) = (tf.position, tf.rotation);
        let com = geom::to_world(pos, rot, local_com(body));
        let mast = geom::to_world(pos, rot, sail.mast);
        let app = geom::rotate_inv(rot, geom::sub(sail.wind, velocity_at(&v, com, mast)));
        let flat: V3 = [app[0], 0.0, app[2]];
        let aws = math::sqrt(flat[0] * flat[0] + flat[2] * flat[2]);
        // Where the wind comes from, off the bow, positive over the starboard side; and where the
        // boom would stream, from the centreline aft, positive to starboard.
        let (awa, free) = if aws > 0.0 {
            (
                math::atan2(-flat[0], flat[2]),
                math::atan2(flat[0], flat[2]),
            )
        } else {
            (0.0, 0.0)
        };
        let limit = boat.sheet_now * BOOM_MAX_DEG.to_radians();
        let boom = clamp(free, -limit, limit);
        let attack = (free.abs() - boom.abs()).to_degrees();
        let (s, c) = math::sin_cos(boom);
        let normal = [c, 0.0, -s];
        let along = [s, 0.0, c];
        let open = smoothstep((awa.abs().to_degrees() - NO_GO_HALF_DEG) / NO_GO_RAMP_DEG);
        let push = signed_sq(geom::dot(flat, normal)) * sail.coef * boat.hoist_now * open;
        let effort = geom::add(
            geom::add(sail.mast, geom::scale(along, 0.5 * sail.boom)),
            [0.0, sail.rise, 0.0],
        );
        if let Some(mut e) = ext
            && push != 0.0
        {
            let force = geom::rotate(rot, geom::scale(normal, push));
            e.add_at(force, geom::to_world(pos, rot, effort), com);
        }
        boat.aws = aws;
        boat.awa_deg = awa.to_degrees();
        boat.boom_deg = boom.to_degrees();
        boat.drive = if boat.hoist_now > 0.0 {
            drive_share(free.abs(), boom.abs())
        } else {
            0.0
        };
        boat.trim = if boat.hoist_now <= 0.0 {
            Trim::Furled
        } else if attack < LUFF_DEG {
            Trim::Luffing
        } else if attack > STALL_DEG && boat.sheet_now < 1.0 {
            // Stalled with sheet still to ease; eased right out, a sail square to the wind is
            // running, not overtrimmed.
            Trim::Overtrimmed
        } else {
            Trim::Good
        };
    }
}

type HullItem<'a> = (
    &'a EntityId,
    &'a Transform,
    Option<&'a Velocity>,
    Option<&'a RigidBody>,
    Option<&'a crate::body::Collider>,
    &'a mut Boat,
    &'a Hull,
    Option<&'a mut ExternalForce>,
);
type HullRegion<'a> = (
    &'a EntityId,
    Option<&'a Transform>,
    &'a crate::water::WaterRegion,
    Option<&'a crate::water::resident::WaterResident>,
);
struct HullFlow {
    at: V3,
    relative: V3,
    wet: f64,
    water_present: bool,
    reaction: Option<(bevy_ecs::entity::Entity, [f64; 2])>,
}

/// `physics.hull`: exact keel/rudder/centre water queries at the current pose. Covered field
/// points use their current-tick columns, including dry ownership; only uncovered points use Sea.
pub(crate) fn hull(
    clock: Res<SimClock>,
    seas: Query<(&EntityId, &Sea)>,
    regions: Query<HullRegion<'static>>,
    samples: Res<crate::water::buoyancy::Samples>,
    mut feedbacks: Query<(
        bevy_ecs::entity::Entity,
        &EntityId,
        &mut crate::water::feedback::WaterFeedback,
    )>,
    mut boats: Query<HullItem<'static>>,
) -> Result<(), amoris_contract::Problem> {
    use crate::water::buoyancy::{Cover, HullPoint, Point, hull_failed};
    let t = start_time(&clock);
    let dt = clock.dt();
    let field = sea_field(&seas);
    let feedback_index: std::collections::BTreeMap<_, _> =
        by_id(&feedbacks).map(|(e, id, _)| (*id, e)).collect();
    let regions: Vec<_> = by_id(&regions)
        .filter_map(|(id, tf, region, state)| {
            Cover::new(*id, tf.map_or([0.0; 3], |t| t.position), region)
                .map(|cover| (cover, region, state))
        })
        .collect();
    for (id, tf, vel, body, collider, mut boat, hull, ext) in by_id_mut(&mut boats) {
        boat.rudder_now = follow(
            boat.rudder_now,
            clamp(boat.rudder, -1.0, 1.0),
            RUDDER_RATE * dt,
        );
        let Some(mut e) = ext else {
            continue;
        };
        if field.is_none() && regions.is_empty() {
            continue;
        }
        let v = vel.copied().unwrap_or_default();
        let (pos, rot) = (tf.position, tf.rotation);
        let com = geom::to_world(pos, rot, local_com(body));
        let flow = |role: HullPoint| -> Result<HullFlow, amoris_contract::Problem> {
            let at = geom::to_world(pos, rot, role.local(hull));
            let owner = regions.iter().find_map(|(cover, region, state)| {
                cover.grid_at(at).map(|grid| (cover, *region, *state, grid))
            });
            let (water, bed, water_present, reaction) = if let Some((cover, region, state, grid)) =
                owner
            {
                let coupled = feedback_index.contains_key(&cover.id);
                let column = if cover.resident {
                    let state = state
                        .ok_or_else(|| hull_failed("resident Hull field has no completed state"))?;
                    if state.completed_tick != clock.tick.0 {
                        return Err(hull_failed(
                            "resident Hull field has not completed this tick",
                        ));
                    }
                    let point = Point::hull(*id, role, *tf, at, grid, coupled, cover)?;
                    samples
                        .get_hull(&point, cover.id, &state.recipe, clock.tick.0)
                        .ok_or_else(|| {
                            hull_failed("resident Hull has no current-pose field sample")
                        })?
                } else {
                    crate::water::buoyancy::sample(region, grid)
                };
                let reaction = feedback_index.get(&cover.id).map(|e| (*e, grid));
                (
                    Some(crate::sea::Water {
                        height: column[0],
                        velocity: [column[2], 0.0, column[3]],
                    }),
                    Some(column[0] - column[1]),
                    column[1] > 0.0 && at[1] + 0.1 > column[0] - column[1],
                    reaction,
                )
            } else {
                (
                    field.as_ref().map(|f| f.at(at[0], at[2], t)),
                    None,
                    field.is_some(),
                    None,
                )
            };
            let Some(water) = water else {
                return Ok(HullFlow {
                    at,
                    relative: geom::ZERO,
                    wet: 0.0,
                    water_present,
                    reaction,
                });
            };
            let rel = geom::sub(water.velocity, velocity_at(&v, com, at));
            // The original wet ramp is the share of [point.y, point.y + 0.1] below water.
            // A field clips that same layer to its actual [bed, surface] water slab.
            let mut wet = clamp((water.height - at[1]) / 0.1, 0.0, 1.0);
            if let Some(bed) = bed {
                wet -= clamp((bed - at[1]) / 0.1, 0.0, 1.0);
            }
            Ok(HullFlow {
                at,
                relative: geom::rotate_inv(rot, rel),
                wet,
                water_present,
                reaction,
            })
        };
        // Resolve and authenticate all three observations before adding any force or reaction.
        let keel = flow(HullPoint::Keel)?;
        let rudder = flow(HullPoint::Rudder)?;
        let center = flow(HullPoint::Center)?;
        let can_react =
            collider.is_some() && body.is_some_and(|b| b.kind == crate::body::BodyKind::Dynamic);
        let mut apply = |local: V3, point: &HullFlow| -> Result<(), amoris_contract::Problem> {
            let force = geom::rotate(rot, local);
            e.add_at(force, point.at, com);
            if can_react && let Some((target, grid)) = point.reaction {
                let (_, _, mut feedback) = feedbacks
                    .get_mut(target)
                    .map_err(|_| hull_failed("Hull reaction target disappeared"))?;
                feedback.queue(clock.tick.0, grid, [-dt * force[0], -dt * force[2]])?;
            }
            Ok(())
        };
        apply(
            [
                hull.keel_coef * keel.wet * signed_sq(keel.relative[0]),
                0.0,
                0.0,
            ],
            &keel,
        )?;
        let angle = boat.rudder_now * hull.rudder_max_deg.to_radians();
        let (s, c) = math::sin_cos(angle);
        let blade = [c, 0.0, -s];
        let push = hull.rudder_coef * rudder.wet * signed_sq(geom::dot(rudder.relative, blade));
        apply(geom::scale(blade, push), &rudder)?;
        // Centre is the hull resistance's resultant application point, not a literal foil.
        // Keel wetness drives resistance even if the floating hull centre is above the surface.
        // Its own column must still contain supported water: never borrow drag across dry bed.
        let drag_wet = if center.water_present { keel.wet } else { 0.0 };
        apply(
            [
                0.0,
                0.0,
                hull.drag_coef * drag_wet * signed_sq(center.relative[2]),
            ],
            &center,
        )?;
    }
    Ok(())
}
