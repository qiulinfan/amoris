//! The showcase's forces as engine systems (charter 2.4.1, 4.3; simulation.md 4.4 rows 4 to 7):
//! `physics.wind`, `physics.buoyancy`, `physics.sail` and `physics.hull`. Each reads the state at
//! the tick's start (the components `physics.step` wrote back, and any write since), computes in
//! `f64` with `pocket_sim::math`, and adds to each body's `ExternalForce`, which `physics.step`
//! hands the solver. Bodies are visited in `EntityId` order. The sea and the wind are evaluated at
//! the tick's start, `(n - 1) / rate` for tick n.

use bevy_ecs::prelude::{Query, Res};
use pocket_sim::math::{self, clamp};
use pocket_sim::order::{by_id, by_id_mut};
use pocket_sim::{EntityId, SimClock, Tick};

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
    &'a mut Floater,
    Option<&'a mut ExternalForce>,
);

/// `physics.buoyancy`: Archimedes at each buoyancy point under the surface (the point displaces
/// its volume times how far under it is, ramped over its layer) and drag toward the water's own
/// velocity there; `Floater::submerged` is written for every floater.
pub(crate) fn buoyancy(
    clock: Res<SimClock>,
    seas: Query<(&EntityId, &Sea)>,
    mut floaters: Query<FloaterItem<'static>>,
) {
    let t = start_time(&clock);
    let field = sea_field(&seas);
    for (_, tf, vel, body, mut f, ext) in by_id_mut(&mut floaters) {
        let Some(field) = &field else {
            f.submerged = 0.0;
            continue;
        };
        let v = vel.copied().unwrap_or_default();
        let com = geom::to_world(tf.position, tf.rotation, local_com(body));
        let mut gathered = ExternalForce::default();
        let (mut wet, mut total) = (0.0, 0.0);
        for p in &f.points {
            total += p.volume;
            let at = geom::to_world(tf.position, tf.rotation, p.at);
            let water = field.at(at[0], at[2], t);
            let depth = water.height - at[1];
            let under = if p.size > 0.0 {
                clamp(depth / p.size + 0.5, 0.0, 1.0)
            } else if depth > 0.0 {
                1.0
            } else {
                0.0
            };
            if under <= 0.0 {
                continue;
            }
            let displaced = p.volume * under;
            wet += displaced;
            let k = field.density * displaced;
            let rel = geom::sub(water.velocity, velocity_at(&v, com, at));
            let force = [
                rel[0] * f.drag * k,
                field.density * GRAVITY * displaced + rel[1] * f.heave * k,
                rel[2] * f.drag * k,
            ];
            gathered.add_at(force, at, com);
        }
        f.submerged = if total > 0.0 { wet / total } else { 0.0 };
        if let Some(mut e) = ext {
            e.force = geom::add(e.force, gathered.force);
            e.torque = geom::add(e.torque, gathered.torque);
        }
    }
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
    &'a mut Boat,
    &'a Hull,
    Option<&'a mut ExternalForce>,
);

/// `physics.hull`: the rudder blade follows its control; the keel and the rudder, flat plates in
/// the water's flow past them (each acting as far as it is under the surface), and quadratic
/// drag along the hull.
pub(crate) fn hull(
    clock: Res<SimClock>,
    seas: Query<(&EntityId, &Sea)>,
    mut boats: Query<HullItem<'static>>,
) {
    let t = start_time(&clock);
    let dt = clock.dt();
    let field = sea_field(&seas);
    for (_, tf, vel, body, mut boat, hull, ext) in by_id_mut(&mut boats) {
        boat.rudder_now = follow(
            boat.rudder_now,
            clamp(boat.rudder, -1.0, 1.0),
            RUDDER_RATE * dt,
        );
        let (Some(field), Some(mut e)) = (&field, ext) else {
            continue;
        };
        let v = vel.copied().unwrap_or_default();
        let (pos, rot) = (tf.position, tf.rotation);
        let com = geom::to_world(pos, rot, local_com(body));
        // The water's flow past a point of the hull, in the boat's frame, and how far the point is
        // under (0 dry to 1 a decimetre or more down).
        let flow = |local: V3| {
            let at = geom::to_world(pos, rot, local);
            let w = field.at(at[0], at[2], t);
            let rel = geom::sub(w.velocity, velocity_at(&v, com, at));
            (
                at,
                geom::rotate_inv(rot, rel),
                clamp((w.height - at[1]) / 0.1, 0.0, 1.0),
            )
        };
        let (keel_at, keel_flow, keel_wet) = flow(hull.keel_at);
        let keel = [
            hull.keel_coef * keel_wet * signed_sq(keel_flow[0]),
            0.0,
            0.0,
        ];
        e.add_at(geom::rotate(rot, keel), keel_at, com);

        let angle = boat.rudder_now * hull.rudder_max_deg.to_radians();
        let (s, c) = math::sin_cos(angle);
        let blade = [c, 0.0, -s];
        let (rudder_at, rudder_flow, rudder_wet) = flow(hull.rudder_at);
        let push = hull.rudder_coef * rudder_wet * signed_sq(geom::dot(rudder_flow, blade));
        e.add_at(geom::rotate(rot, geom::scale(blade, push)), rudder_at, com);

        // The hull's resistance acts while its keel is in the water.
        let (hull_at, hull_flow, _) = flow(geom::ZERO);
        let drag = [
            0.0,
            0.0,
            hull.drag_coef * keel_wet * signed_sq(hull_flow[2]),
        ];
        e.add_at(geom::rotate(rot, drag), hull_at, com);
    }
}
