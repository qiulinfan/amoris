//! The showcase's pieces as component bundles (charter 2.4.1): the Sloop, a floating crate, the
//! island sea and a breeze, ported from the physics spike (`spikes/physics/src/scene.rs`) and
//! tuned to sail master's `sailboat` task (five units along +x in 300 ticks before a 6 m/s wind,
//! afloat). Units are SI: water of 1000 kg/m^3 and a 450 kg boat, the spike's toy masses and
//! coefficients scaled by 500 together, so it moves as the spike's did.
//!
//! The Sloop's hull is a compound of convex pieces between five stations, a keel fin and a rudder
//! blade, without a density: the body states master's Boat's mass properties (numeric.md 5), with
//! the centre of mass low (ballast in the keel) so it rights itself. Forward is -z.

use bevy_ecs::prelude::Bundle;

use crate::boat::{Boat, BuoyPoint, Floater, Hull, Sail};
use crate::body::{
    Collider, ExternalForce, MassProps, Part, PartShape, RigidBody, Shape, Transform, Velocity,
};
use crate::geom::{self, V3};
use crate::sea::{Sea, Wind};

/// One hull station: where it stands along the boat (z), its bottom and deck heights, and its half
/// widths at the bottom and at the deck.
struct Station {
    z: f64,
    bottom: f64,
    deck: f64,
    half_bottom: f64,
    half_deck: f64,
}

const STATIONS: [Station; 5] = [
    Station {
        z: -1.75,
        bottom: 0.05,
        deck: 0.40,
        half_bottom: 0.01,
        half_deck: 0.03,
    },
    Station {
        z: -1.0,
        bottom: -0.20,
        deck: 0.36,
        half_bottom: 0.12,
        half_deck: 0.30,
    },
    Station {
        z: 0.0,
        bottom: -0.25,
        deck: 0.35,
        half_bottom: 0.18,
        half_deck: 0.40,
    },
    Station {
        z: 1.0,
        bottom: -0.22,
        deck: 0.35,
        half_bottom: 0.16,
        half_deck: 0.38,
    },
    Station {
        z: 1.65,
        bottom: -0.15,
        deck: 0.36,
        half_bottom: 0.12,
        half_deck: 0.32,
    },
];

/// The Sloop's mass, kg.
pub const SLOOP_MASS: f64 = 450.0;
/// Water's density, kg/m^3.
pub const WATER_DENSITY: f64 = 1000.0;

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn section(s: &Station) -> [V3; 4] {
    [
        [-s.half_bottom, s.bottom, s.z],
        [s.half_bottom, s.bottom, s.z],
        [-s.half_deck, s.deck, s.z],
        [s.half_deck, s.deck, s.z],
    ]
}

/// A station's cross-section area: a trapezoid.
fn area(s: &Station) -> f64 {
    (s.half_bottom + s.half_deck) * (s.deck - s.bottom)
}

/// The convex pieces: one between each pair of stations, the keel fin and the rudder blade.
fn hull_parts() -> Vec<Part> {
    let mut pieces: Vec<Vec<V3>> = STATIONS
        .windows(2)
        .map(|pair| {
            let mut pts = section(&pair[0]).to_vec();
            pts.extend_from_slice(&section(&pair[1]));
            pts
        })
        .collect();
    // The keel: a fin under the middle, 0.85 long, reaching 0.6 below the bottom, tapering down.
    pieces.push(vec![
        [-0.03, -0.24, -0.45],
        [0.03, -0.24, -0.45],
        [-0.03, -0.24, 0.40],
        [0.03, -0.24, 0.40],
        [-0.04, -0.85, -0.25],
        [0.04, -0.85, -0.25],
        [-0.04, -0.85, 0.30],
        [0.04, -0.85, 0.30],
    ]);
    // The rudder blade behind the transom (its collider stays straight; its force turns with it).
    pieces.push(vec![
        [-0.015, 0.0, 1.66],
        [0.015, 0.0, 1.66],
        [-0.015, 0.0, 1.90],
        [0.015, 0.0, 1.90],
        [-0.015, -0.55, 1.66],
        [0.015, -0.55, 1.66],
        [-0.015, -0.55, 1.86],
        [0.015, -0.55, 1.86],
    ]);
    pieces
        .into_iter()
        .map(|points| Part {
            position: geom::ZERO,
            rotation: geom::IDENTITY,
            shape: PartShape::ConvexHull { points },
        })
        .collect()
}

/// Buoyancy points inside each hull piece: a 3 x 2 x 3 lattice (along, across, up) between its
/// two stations, each an equal share of the piece's volume (the prismatoid rule, exact for the
/// loft between two trapezoids), and two in the keel.
pub fn hull_points() -> Vec<BuoyPoint> {
    let (nu, nv, nw) = (3u32, 2u32, 3u32);
    let cells = f64::from(nu * nv * nw);
    let mut out = Vec::new();
    for pair in STATIONS.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let mid = Station {
            z: lerp(a.z, b.z, 0.5),
            bottom: lerp(a.bottom, b.bottom, 0.5),
            deck: lerp(a.deck, b.deck, 0.5),
            half_bottom: lerp(a.half_bottom, b.half_bottom, 0.5),
            half_deck: lerp(a.half_deck, b.half_deck, 0.5),
        };
        let volume = (b.z - a.z) / 6.0 * (area(a) + 4.0 * area(&mid) + area(b));
        let share = volume / cells;
        for u in 0..nu {
            let t = (f64::from(u) + 0.5) / f64::from(nu);
            let (z, bottom, deck) = (
                lerp(a.z, b.z, t),
                lerp(a.bottom, b.bottom, t),
                lerp(a.deck, b.deck, t),
            );
            let (hb, hd) = (
                lerp(a.half_bottom, b.half_bottom, t),
                lerp(a.half_deck, b.half_deck, t),
            );
            for w in 0..nw {
                let s = (f64::from(w) + 0.5) / f64::from(nw);
                let y = lerp(bottom, deck, s);
                let half = lerp(hb, hd, s);
                for v in 0..nv {
                    let x = lerp(-half, half, (f64::from(v) + 0.5) / f64::from(nv));
                    out.push(BuoyPoint {
                        at: [x, y, z],
                        volume: share,
                        size: (deck - bottom) / f64::from(nw),
                    });
                }
            }
        }
    }
    // The keel displaces a little water too.
    for z in [-0.15, 0.15] {
        out.push(BuoyPoint {
            at: [0.0, -0.55, z],
            volume: 0.012,
            size: 0.3,
        });
    }
    out
}

/// The Sloop's components at `position`, its bow toward `heading_deg` (0 toward -z, 90 toward +x),
/// sail furled (`Boat::hoist` 0) and the sheet eased out.
pub type SloopBundle = (
    Transform,
    Velocity,
    RigidBody,
    Collider,
    ExternalForce,
    Floater,
    Boat,
    Sail,
    Hull,
);

pub fn sloop(position: V3, heading_deg: f64) -> SloopBundle {
    let body = RigidBody::dynamic().with_mass(MassProps::solid_box(
        SLOOP_MASS,
        [0.32, 0.3, 1.6],
        [0.0, -0.35, 0.0],
    ));
    let collider = Collider::new(Shape::Compound {
        parts: hull_parts(),
    })
    .with_friction(0.4, 0.1);
    let floater = Floater {
        points: hull_points(),
        drag: 0.05,
        heave: 6.0,
        submerged: 0.0,
    };
    (
        Transform::at_yaw(position, -heading_deg),
        Velocity::default(),
        body,
        collider,
        ExternalForce::default(),
        floater,
        Boat::default(),
        Sail::default(),
        Hull::default(),
    )
}

/// A floating crate: a 0.6 m cube of about 76 kg, turned `yaw_deg`.
pub fn crate_box(position: V3, yaw_deg: f64) -> impl Bundle {
    let h = [0.3; 3];
    (
        Transform::at_yaw(position, yaw_deg),
        Velocity::default(),
        RigidBody::dynamic(),
        Collider::new(Shape::Cuboid { half_extents: h })
            .with_density(350.0)
            .with_friction(0.5, 0.1),
        ExternalForce::default(),
        Floater::solid_box(h),
    )
}

/// Master's island sea in water of 1000 kg/m^3.
pub fn island_sea() -> Sea {
    Sea::island(WATER_DENSITY)
}

/// A calm sea at level 0.
pub fn calm_sea() -> Sea {
    Sea {
        density: WATER_DENSITY,
        ..Sea::default()
    }
}

/// A wind from `from_deg` at `speed`, steady.
pub fn breeze(from_deg: f64, speed: f64) -> Wind {
    Wind::steady(from_deg, speed)
}
