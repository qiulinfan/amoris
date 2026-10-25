//! The showcase's forces as engine systems (charter 2.4.1, 4.3; simulation.md 4.4 rows 4 to 7):
//! `physics.wind`, `physics.buoyancy`, `physics.sail` and `physics.hull`. Each reads the state at
//! the tick's start (the components `physics.step` wrote back, and any write since), computes in
//! `f64` with `amoris_sim::math`, and adds to each body's `ExternalForce`, which `physics.step`
//! hands the solver. Bodies are visited in `EntityId` order. The sea and the wind are evaluated at
//! the tick's start, `(n - 1) / rate` for tick n.

use amoris_assets::water_field::MAX_POINT_RADIUS;
use amoris_sim::math::{self, clamp};
use amoris_sim::order::{by_id, by_id_mut};
use amoris_sim::{EntityId, SimClock, Tick};
use bevy_ecs::prelude::{Query, Res};

use crate::boat::{
    BOOM_MAX_DEG, Boat, Floater, HOIST_RATE, Hull, LUFF_DEG, NO_GO_HALF_DEG, NO_GO_RAMP_DEG,
    RUDDER_RATE, SHEET_RATE, STALL_DEG, Sail, Trim,
};
use crate::body::{ExternalForce, RigidBody, Transform, Velocity, body_mass, local_com};
use crate::geom::{self, V3};
use crate::sea::{GRAVITY, Sea, WaveField, Wind};
use crate::solver::velocity_at;
use crate::water::displacement::head_per_m3;

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
pub(crate) fn sea_field(seas: &Query<(&EntityId, &Sea)>) -> Option<WaveField> {
    by_id(seas).next().map(|(_, s)| s.field())
}

/// The water at one Floater point: its surface and velocity there (in a field, with the free
/// surface's vertical motion, falling linearly to nothing at the bed as in a long wave), its
/// density, the bed under a field point and the free surface's slope (zero on the open sea).
pub(crate) struct PointWater {
    pub water: crate::sea::Water,
    pub density: f64,
    pub bed: Option<f64>,
    pub slope: [f64; 2],
    /// The free surface's rate of rise there, m/s (zero on the open sea, evaluated each tick).
    pub rise: f64,
    /// The covering region and the point's grid position there.
    pub cover: Option<(EntityId, [f64; 2])>,
    /// The covering region's column size, m (1 on the open sea).
    pub cell: f64,
}

/// Where a Floater point finds water: the first covering region (its current-tick resident
/// sample, or the CPU field) and otherwise the analytic sea. `None`: a dry column or no water.
#[allow(clippy::too_many_arguments)]
pub(crate) fn point_water(
    id: EntityId,
    point: usize,
    at: V3,
    regions: &[(crate::water::buoyancy::Cover, &crate::water::WaterRegion)],
    samples: &crate::water::buoyancy::Samples,
    field: &Option<WaveField>,
    t: f64,
    tick: u64,
) -> Result<Option<PointWater>, amoris_contract::Problem> {
    let region = regions
        .iter()
        .find_map(|(cover, r)| cover.grid_at(at).map(|grid| (cover, grid, *r)));
    if let Some((cover, grid, region)) = region {
        let column = if region.solver == crate::water::Solver::GpuResident {
            samples
                .get((id, point), at, cover.id, tick)
                .ok_or_else(|| {
                    crate::water::resident::failed("resident buoyancy has no current-pose sample")
                })?
        } else {
            crate::water::buoyancy::sample(region, grid)
        };
        if column[1] <= 0.0 {
            return Ok(None);
        }
        let lift = column[6] + column[2] * column[4] + column[3] * column[5];
        let height = clamp((at[1] - (column[0] - column[1])) / column[1], 0.0, 1.0);
        return Ok(Some(PointWater {
            water: crate::sea::Water {
                height: column[0],
                velocity: [column[2], lift * height, column[3]],
            },
            density: 1000.0,
            bed: Some(column[0] - column[1]),
            slope: [column[4], column[5]],
            rise: column[6],
            cover: Some((cover.id, grid)),
            cell: cover.cell(),
        }));
    }
    Ok(field.as_ref().map(|field| PointWater {
        water: field.at(at[0], at[2], t),
        density: field.density,
        bed: None,
        slope: [0.0; 2],
        rise: 0.0,
        cover: None,
        cell: 1.0,
    }))
}

/// The radius of the water a buoyancy point pushes aside, m. The step spreads the volume with
/// weights (1 - d^2/R^2)^2, whose peak column takes the volume over pi R^2 / 3; at R = 2 sqrt(A)
/// for the waterplane A its layer stands for (volume over thickness), the head under the point
/// rises at a quarter of the rate it sinks, which the point feels in the same step
/// (`physics.buoyancy`'s own head).
pub(crate) fn footprint(volume: f64, size: f64) -> f64 {
    let waterplane = if size > 0.0 {
        volume / size
    } else {
        math::pow(volume, 2.0 / 3.0)
    };
    2.0 * math::sqrt(waterplane)
}

/// How much of a buoyancy layer `size` thick, centred at height `y`, stands in the water: below
/// the surface at `top` and, over a field, above the bed.
pub(crate) fn immersion(y: f64, size: f64, top: f64, bed: Option<f64>) -> f64 {
    let layer = |top: f64| {
        let depth = top - y;
        if size > 0.0 {
            clamp(depth / size + 0.5, 0.0, 1.0)
        } else if depth > 0.0 {
            1.0
        } else {
            0.0
        }
    };
    let mut under = layer(top);
    if let Some(bed) = bed {
        under -= layer(bed);
    }
    under
}

/// One Floater point in the water this tick (`physics.buoyancy`'s first pass).
struct Wet {
    at: V3,
    here: PointWater,
    moving: V3,
    /// How far under it stands now, and where it will stand at the step's end against the free
    /// surface then.
    now: f64,
    under: f64,
    /// In a region with WaterDisplacement: the region's columns and the point's footprint radius
    /// in columns, as queued.
    spread: Option<([u32; 2], f64)>,
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
/// velocity there; `Floater::submerged` is written for every floater. In a water region the
/// pressure acts across the free surface's slope (Froude-Krylov, water.md 47), the water moves
/// up and down with its surface, and a region with WaterDisplacement receives each point's
/// displaced volume for its next step. A floater with added mass shares each point's forces with
/// the water it carries and brakes as it enters (water.md 56).
#[allow(clippy::too_many_arguments)]
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
    mut displacements: Query<(
        bevy_ecs::entity::Entity,
        &EntityId,
        &mut crate::water::displacement::WaterDisplacement,
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
    let displacement_index: std::collections::BTreeMap<_, _> =
        by_id(&displacements).map(|(e, id, _)| (*id, e)).collect();
    let regions: Vec<_> = by_id(&regions)
        .filter_map(|(id, t, r)| {
            crate::water::buoyancy::Cover::new(*id, t.map_or([0.0; 3], |t| t.position), r)
                .map(|cover| (cover, r))
        })
        .collect();
    let dt = clock.dt();
    for (id, tf, vel, body, collider, mut f, ext) in by_id_mut(&mut floaters) {
        let v = vel.copied().unwrap_or_default();
        let com = geom::to_world(tf.position, tf.rotation, local_com(body));
        let mut gathered = ExternalForce::default();
        let (mut wet, mut total) = (0.0, 0.0);
        let volume: f64 = f.points.iter().map(|p| p.volume).sum();
        let mass = body_mass(body, collider).filter(|m| *m > 0.0 && volume > 0.0);
        let mut points = Vec::with_capacity(f.points.len());
        for (point, p) in f.points.iter().enumerate() {
            total += p.volume;
            let at = geom::to_world(tf.position, tf.rotation, p.at);
            let Some(here) =
                point_water(*id, point, at, &regions, &samples, &field, t, clock.tick.0)?
            else {
                points.push(None);
                continue;
            };
            let moving = velocity_at(&v, com, at);
            let now = immersion(at[1], p.size, here.water.height, here.bed);
            // The hydrostatic spring of a light float is stiff for one tick: it is evaluated where
            // the point will stand at the step's end, against the free surface then, linearly
            // implicit as in D. Baraff and A. Witkin, "Large Steps in Cloth Simulation", SIGGRAPH
            // 1998. Sampled at the tick's start, its one-sided contact gains energy on every
            // bounce.
            let top = here.water.height + dt * here.rise;
            let under = immersion(at[1] + dt * moving[1], p.size, top, here.bed);
            let spread = here
                .cover
                .filter(|(region, _)| displacement_index.contains_key(region))
                .and_then(|(region, _)| regions.iter().find(|(c, _)| c.id == region))
                .map(|(cover, _)| {
                    // Deeper in the water a body's volume lifts the surface over a wider patch:
                    // the potential flow of a source a depth d down reaches the surface across
                    // about d.
                    let deep = math::max(here.water.height - at[1] - 0.5 * p.size, 0.0);
                    let radius = (footprint(p.volume, p.size) + deep) / here.cell;
                    (cover.size(), math::min(radius, MAX_POINT_RADIUS))
                });
            points.push(Some(Wet {
                at,
                here,
                moving,
                now,
                under,
                spread,
            }));
        }
        // The head the body's own change of displaced volume adds over the step: the water holds
        // it through the step, and this tick's sample does not show it yet. Felt a tick late,
        // that push is a late spring, and it pumped the water the body bobs in (water.md 56).
        let heads: Vec<f64> = points
            .iter()
            .map(|entry| {
                let Some(Wet {
                    here,
                    spread: Some((size, _)),
                    ..
                }) = entry
                else {
                    return 0.0;
                };
                let Some((region, grid)) = here.cover else {
                    return 0.0;
                };
                points
                    .iter()
                    .zip(&f.points)
                    .filter_map(|(other, p)| {
                        let o = other.as_ref()?;
                        let (_, radius) = o.spread?;
                        let (from, source) = o.here.cover?;
                        (from == region).then(|| {
                            p.volume
                                * (o.under - o.now)
                                * head_per_m3(source, radius, grid, *size, here.cell)
                        })
                    })
                    .sum()
            })
            .collect();
        for ((entry, p), head) in points.iter().zip(&f.points).zip(heads) {
            let Some(Wet {
                at,
                here,
                moving,
                now,
                under,
                spread,
            }) = entry
            else {
                continue;
            };
            let (at, moving, now) = (*at, *moving, *now);
            let under = if head == 0.0 {
                *under
            } else {
                let top = here.water.height + dt * here.rise + head;
                immersion(at[1] + dt * moving[1], p.size, top, here.bed)
            };
            if under <= 0.0 {
                continue;
            }
            let reaction = here
                .cover
                .and_then(|(region, grid)| feedback_index.get(&region).map(|e| (*e, grid)));
            let pushes = here
                .cover
                .and_then(|(region, grid)| displacement_index.get(&region).map(|e| (*e, grid)));
            let (water, density, slope) = (here.water, here.density, here.slope);
            let displaced = p.volume * under;
            wet += displaced;
            let k = density * displaced;
            let rel = geom::sub(water.velocity, moving);
            // The point's share of the body's mass, and the water it carries along: the added
            // mass of the inertia term of J. R. Morison, M. P. O'Brien, J. W. Johnson and S. A.
            // Schaaf, "The force exerted by surface waves on piles", Petroleum Transactions, AIME
            // 189, 1950 (potential flow gives a sphere half its displaced mass).
            let share = mass.map(|m| m * p.volume / volume);
            let carried = share.map_or(0.0, |_| f.added_mass * k);
            // Entering, as the point sinks or the surface rises over it, the point gains carried
            // water at d(m_a)/dt and gives it its vertical velocity relative to the water: the
            // momentum theory of water entry, T. von Kármán, "The impact on seaplane floats during
            // landing", NACA Technical Note 321, 1929 (the force d(m_a v)/dt; H. Wagner, ZAMM
            // 12(4), 1932, adds the water's pile-up). The water carries that momentum off as spray
            // and waves; leaving, the point sheds its water. Gliding along the surface carries no
            // new water: a board's planing is its own (`physics.board`).
            let entering = if share.is_some() && f.added_mass > 0.0 {
                f.added_mass * density * p.volume * math::max(under - now, 0.0) / dt
            } else {
                0.0
            };
            // Drag, heave damping and the entry's braking are implicit over the point's inertia:
            // c / (1 + c dt / m), so one tick never reverses the water's relative velocity.
            let implicit = |c: f64| match share {
                Some(m) => c / (1.0 + c * dt / (m + carried)),
                None => c,
            };
            // Hydrostatic pressure under a sloping free surface pushes down its slope:
            // -rho g V grad(eta) (Froude-Krylov in the long-wave limit). Zero on the open sea.
            let lift = density * GRAVITY * displaced;
            let force = [
                rel[0] * implicit(f.drag * k) - lift * slope[0],
                lift + rel[1] * implicit(f.heave * k + entering),
                rel[2] * implicit(f.drag * k) - lift * slope[1],
            ];
            // The point and its carried water accelerate together under these forces, the carried
            // water's own (it follows the flow, at -g grad(eta) along the surface in the
            // long-wave limit) and the body's weight alone: the carried water weighs nothing. The
            // engine applies m g, which leaves over the body's m:
            // F m / (m + m_a) + m m_a / (m + m_a) (a_water + g up).
            let force = match share {
                Some(m) if carried > 0.0 => {
                    let held = carried * GRAVITY;
                    let total = m + carried;
                    [
                        (force[0] - held * slope[0]) * m / total,
                        (force[1] + held) * m / total,
                        (force[2] - held * slope[1]) * m / total,
                    ]
                }
                _ => force,
            };
            gathered.add_at(force, at, com);
            if let Some((e, grid)) = pushes
                && let Some((_, radius)) = spread
                && let Ok((_, _, mut target)) = displacements.get_mut(e)
            {
                target.queue(clock.tick.0, grid, displaced, *radius)?;
            }
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
