//! The plain-data components a rigid body is made from (docs/spec/architecture.md 4.4): where it is,
//! how it moves, what kind of body it is and its mass, its collider, and the force the force
//! systems gather for it. All `f64` (numeric.md 3.1); the solver's state is a cache built from them
//! in `EntityId` order (`solver`).
//!
//! The mass rule of numeric.md 5: Rapier computes the mass properties of a convex hull, a triangle
//! mesh or a compound with the platform's math natively, so such a collider on a dynamic body
//! carries no density, and the body states its mass properties instead; balls, cuboids and
//! capsules, whose mass properties are closed formulas, may carry a density. A body never gets
//! mass from both, since Rapier sums two masses through an eigen decomposition that calls the
//! platform's math too.

use bevy_ecs::prelude::Component;
use pocket_contract::{Problem, detail};
use pocket_sim::EntityId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::geom::{self, Q4, V3};

/// Where an entity is: its origin and orientation in the world (metres; a unit quaternion
/// `[x, y, z, w]`). Physics writes it back after every step for the bodies it moves; a write from
/// outside moves the body there at the next step.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Transform {
    /// The origin in the world, metres.
    pub position: V3,
    /// The orientation, a unit quaternion `[x, y, z, w]`.
    pub rotation: Q4,
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            position: geom::ZERO,
            rotation: geom::IDENTITY,
        }
    }
}

impl Transform {
    pub fn at(position: V3) -> Transform {
        Transform {
            position,
            rotation: geom::IDENTITY,
        }
    }

    /// At `position`, turned `yaw_deg` about +y (a boat whose bow is -z heads `-yaw_deg`).
    pub fn at_yaw(position: V3, yaw_deg: f64) -> Transform {
        Transform {
            position,
            rotation: geom::yaw(yaw_deg),
        }
    }
}

/// How a body moves: the velocity of its centre of mass (m/s) and its angular velocity (rad/s),
/// both in the world frame. Written back after every step; a write from outside sets the body's
/// velocity at the next step.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Velocity {
    /// Linear velocity of the centre of mass, m/s.
    pub linear: V3,
    /// Angular velocity, rad/s.
    pub angular: V3,
}

/// What kind of body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum BodyKind {
    /// Moved by forces, gravity and contacts.
    #[default]
    Dynamic,
    /// Never moves (an island, a pier).
    Fixed,
}

/// Mass properties stated outright: the mass (kg), the centre of mass in the body's frame, and the
/// principal moments of inertia about axes through it parallel to the body's axes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MassProps {
    /// kg, above 0.
    pub mass: f64,
    /// The centre of mass in the body's frame, metres.
    pub center: V3,
    /// Principal moments of inertia about the body's axes through the centre of mass, kg m^2.
    pub inertia: V3,
}

impl MassProps {
    /// A box of half extents `h` and `mass`, centred at `center`.
    pub fn solid_box(mass: f64, h: V3, center: V3) -> MassProps {
        let m = mass / 3.0;
        MassProps {
            mass,
            center,
            inertia: [
                m * (h[1] * h[1] + h[2] * h[2]),
                m * (h[0] * h[0] + h[2] * h[2]),
                m * (h[0] * h[0] + h[1] * h[1]),
            ],
        }
    }
}

/// A rigid body, with a `Transform` and a `Collider` on the same entity, and usually a `Velocity`
/// and an `ExternalForce` (a body without them moves as if both were zero and shows neither).
///
/// No component requires another: a restore inserts components section by section, and a required
/// one would come back at its default on an entity of the original world that had none, so the
/// fork would differ from its source (persistence.md, P2). The bundles in [`crate::sailing`] insert
/// every component a body uses.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct RigidBody {
    pub kind: BodyKind,
    /// Stated mass properties. Required on a dynamic body whose collider is a hull, a mesh or a
    /// compound; absent when the collider's density gives the mass.
    pub mass: Option<MassProps>,
    /// Linear velocity damping, per second.
    pub linear_damping: f64,
    /// Angular velocity damping, per second.
    pub angular_damping: f64,
    /// Gravity's multiplier on this body.
    pub gravity_scale: f64,
}

impl Default for RigidBody {
    fn default() -> Self {
        RigidBody {
            kind: BodyKind::Dynamic,
            mass: None,
            linear_damping: 0.0,
            angular_damping: 0.0,
            gravity_scale: 1.0,
        }
    }
}

impl RigidBody {
    pub fn dynamic() -> RigidBody {
        RigidBody::default()
    }

    pub fn fixed() -> RigidBody {
        RigidBody {
            kind: BodyKind::Fixed,
            ..RigidBody::default()
        }
    }

    pub fn with_mass(mut self, mass: MassProps) -> RigidBody {
        self.mass = Some(mass);
        self
    }
}

/// A shape that can be a part of a compound.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum PartShape {
    /// A sphere.
    Ball { radius: f64 },
    /// A box of half extents.
    Cuboid { half_extents: V3 },
    /// A capsule along +y: a segment of half height `half_height` swept by `radius`.
    Capsule { half_height: f64, radius: f64 },
    /// The convex hull of points in the body's frame.
    ConvexHull { points: Vec<V3> },
}

/// One part of a compound: its shape and its pose in the body's frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub position: V3,
    pub rotation: Q4,
    pub shape: PartShape,
}

/// A collider's shape, in the body's frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum Shape {
    Ball {
        radius: f64,
    },
    Cuboid {
        half_extents: V3,
    },
    Capsule {
        half_height: f64,
        radius: f64,
    },
    ConvexHull {
        points: Vec<V3>,
    },
    /// A triangle mesh: vertices and triangles as index triples.
    TriMesh {
        vertices: Vec<V3>,
        indices: Vec<[u32; 3]>,
    },
    /// Several parts, none of them a compound.
    Compound {
        parts: Vec<Part>,
    },
}

impl Shape {
    /// Whether Rapier would compute this shape's mass properties with the platform's math
    /// (numeric.md 5): hulls, meshes and compounds.
    pub fn platform_mass(&self) -> bool {
        matches!(
            self,
            Shape::ConvexHull { .. } | Shape::TriMesh { .. } | Shape::Compound { .. }
        )
    }

    fn name(&self) -> &'static str {
        match self {
            Shape::Ball { .. } => "Ball",
            Shape::Cuboid { .. } => "Cuboid",
            Shape::Capsule { .. } => "Capsule",
            Shape::ConvexHull { .. } => "ConvexHull",
            Shape::TriMesh { .. } => "TriMesh",
            Shape::Compound { .. } => "Compound",
        }
    }
}

/// The collider of the body on the same entity (a `Collider` without a `RigidBody` is fixed).
/// Every entity with a `Collider` and a `Transform` gets a body in the solver; a `Collider` without
/// a `Transform` is refused at the step (`sim.body_invalid`).
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Collider {
    pub shape: Shape,
    /// kg/m^3, giving the body its mass; only on a ball, a cuboid or a capsule of a body without
    /// stated mass properties (numeric.md 5).
    #[serde(default)]
    pub density: Option<f64>,
    /// Coulomb friction, at least 0 (default 0.5).
    #[serde(default = "half")]
    pub friction: f64,
    /// Bounciness, at least 0 (default 0).
    #[serde(default)]
    pub restitution: f64,
}

fn half() -> f64 {
    0.5
}

impl Collider {
    pub fn new(shape: Shape) -> Collider {
        Collider {
            shape,
            density: None,
            friction: 0.5,
            restitution: 0.0,
        }
    }

    pub fn with_density(mut self, density: f64) -> Collider {
        self.density = Some(density);
        self
    }

    pub fn with_friction(mut self, friction: f64, restitution: f64) -> Collider {
        self.friction = friction;
        self.restitution = restitution;
        self
    }
}

/// The force and torque (about the centre of mass) applied to the body over the coming step, in
/// the world frame. Scripts and the force systems add to it during a tick; `physics.step` applies
/// it and sets it back to zero, so it is zero at every boundary unless a boundary write sets it.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ExternalForce {
    /// Newtons.
    pub force: V3,
    /// Newton metres, about the centre of mass.
    pub torque: V3,
}

impl ExternalForce {
    /// Adds `force` applied at the world point `at`, for a body whose centre of mass is `com`.
    pub fn add_at(&mut self, force: V3, at: V3, com: V3) {
        self.force = geom::add(self.force, force);
        self.torque = geom::add(self.torque, geom::cross(geom::sub(at, com), force));
    }
}

/// The centre of mass of a body in its own frame: the stated one, or the shape's centre (a ball,
/// cuboid or capsule given a density is centred on the body's origin).
pub fn local_com(body: Option<&RigidBody>) -> V3 {
    body.and_then(|b| b.mass).map_or(geom::ZERO, |m| m.center)
}

fn invalid(code: &str, entity: EntityId, what: &str, reason: String) -> Problem {
    Problem::new(
        code,
        format!("Entity {entity}: {reason}"),
        detail([("entity", json!(entity.get())), (what, json!(reason))]),
    )
}

// The checks judge each value as the solver receives it, rounded to `f32` (numeric.md 3.1): 1e39
// is finite in `f64` and infinite there, 1e-50 positive in `f64` and 0 there.

/// Positive, finite and normal in `f32`.
fn positive(x: f64) -> bool {
    let s = geom::f32_of(x);
    s.is_finite() && s >= f32::MIN_POSITIVE
}

/// Finite and at least 0 in `f32`.
fn at_least_zero(x: f64) -> bool {
    let s = geom::f32_of(x);
    s.is_finite() && s >= 0.0
}

fn check_part(s: &PartShape) -> Result<(), String> {
    match s {
        PartShape::Ball { radius } if !positive(*radius) => {
            Err("a ball's radius must be above 0 and below 3.4e38 (the solver's f32 range)".into())
        }
        PartShape::Cuboid { half_extents } if !half_extents.iter().all(|h| positive(*h)) => Err(
            "a cuboid's half extents must be above 0 and below 3.4e38 (the solver's f32 range)"
                .into(),
        ),
        PartShape::Capsule {
            half_height,
            radius,
        } if !(positive(*radius) && at_least_zero(*half_height)) => Err(
            "a capsule needs a radius above 0 and a half height of at least 0, both below 3.4e38 \
             (the solver's f32 range)"
                .into(),
        ),
        PartShape::ConvexHull { points }
            if points.len() < 4 || !points.iter().all(|p| geom::finite32(p)) =>
        {
            Err("a convex hull needs at least 4 points, finite in the solver's f32".into())
        }
        _ => Ok(()),
    }
}

fn check_shape(shape: &Shape) -> Result<(), String> {
    match shape {
        Shape::Ball { radius } => check_part(&PartShape::Ball { radius: *radius }),
        Shape::Cuboid { half_extents } => check_part(&PartShape::Cuboid {
            half_extents: *half_extents,
        }),
        Shape::Capsule {
            half_height,
            radius,
        } => check_part(&PartShape::Capsule {
            half_height: *half_height,
            radius: *radius,
        }),
        Shape::ConvexHull { points } => check_part(&PartShape::ConvexHull {
            points: points.clone(),
        }),
        Shape::TriMesh { vertices, indices } => {
            let n = vertices.len();
            if indices.is_empty() || !vertices.iter().all(|p| geom::finite32(p)) {
                return Err(
                    "a triangle mesh needs at least one triangle and vertices finite in \
                            the solver's f32"
                        .into(),
                );
            }
            if indices
                .iter()
                .flatten()
                .any(|&i| usize::try_from(i).map_or(true, |i| i >= n))
            {
                return Err(format!(
                    "a triangle names a vertex past the {n} the mesh has"
                ));
            }
            Ok(())
        }
        Shape::Compound { parts } => {
            if parts.is_empty() {
                return Err("a compound needs at least one part".into());
            }
            for p in parts {
                if !geom::finite32(&p.position) || geom::normalized(p.rotation).is_none() {
                    return Err("a part needs a position finite in the solver's f32 and a \
                                finite, non-zero rotation"
                        .into());
                }
                check_part(&p.shape)?;
            }
            Ok(())
        }
    }
}

/// Checks a body and its collider before the solver builds them: the shape is one Rapier can build,
/// and the mass comes from exactly one allowed place (numeric.md 5). Every value is judged as the
/// solver receives it, rounded to `f32`. Errors: `sim.collider_density` (a hull, mesh or compound
/// given a density), `sim.collider_invalid` (a degenerate or non-finite shape, friction,
/// restitution or density) and `sim.body_invalid` (a dynamic body with no mass, or a mass both
/// stated and from a density, or stated mass properties that are not positive and finite).
pub fn validate(
    entity: EntityId,
    body: Option<&RigidBody>,
    collider: &Collider,
) -> Result<(), Problem> {
    check_shape(&collider.shape)
        .map_err(|r| invalid("sim.collider_invalid", entity, "reason", r))?;
    if !at_least_zero(collider.friction) || !at_least_zero(collider.restitution) {
        return Err(invalid(
            "sim.collider_invalid",
            entity,
            "reason",
            "friction and restitution must be at least 0 and finite in the solver's f32".into(),
        ));
    }
    let dynamic = body.is_some_and(|b| b.kind == BodyKind::Dynamic);
    // Fixed bodies included: Rapier computes every collider's mass properties, whatever its
    // body's kind, and keeps them in the cache (numeric.md 11, slice 1).
    if collider.density.is_some() && collider.shape.platform_mass() {
        return Err(Problem::new(
            "sim.collider_density",
            format!(
                "Entity {entity}: a {} collider cannot take a density, since Rapier would compute \
                 its mass properties with the platform's math (numeric.md 5); give a dynamic \
                 body's RigidBody its mass properties (mass, center, inertia) instead.",
                collider.shape.name()
            ),
            detail([
                ("entity", json!(entity.get())),
                ("collider", json!(collider.shape.name())),
            ]),
        ));
    }
    if let Some(d) = collider.density
        && !positive(d)
    {
        return Err(invalid(
            "sim.collider_invalid",
            entity,
            "reason",
            "a density must be above 0 and below 3.4e38 (the solver's f32 range)".into(),
        ));
    }
    let Some(b) = body else { return Ok(()) };
    if !(at_least_zero(b.linear_damping)
        && at_least_zero(b.angular_damping)
        && geom::finite32(&[b.gravity_scale]))
    {
        return Err(invalid(
            "sim.body_invalid",
            entity,
            "reason",
            "damping must be at least 0, and damping and the gravity scale finite in the solver's \
             f32"
            .into(),
        ));
    }
    if let Some(m) = b.mass {
        let ok =
            positive(m.mass) && geom::finite32(&m.center) && m.inertia.iter().all(|i| positive(*i));
        if !ok {
            return Err(invalid(
                "sim.body_invalid",
                entity,
                "reason",
                "stated mass properties need a mass and inertias above 0 and a centre, all \
                 positive or finite in the solver's f32 (from 1.2e-38 to 3.4e38)"
                    .into(),
            ));
        }
    }
    if dynamic {
        match (b.mass.is_some(), collider.density.is_some()) {
            (false, false) => Err(invalid(
                "sim.body_invalid",
                entity,
                "reason",
                "a dynamic body needs its mass: a density on a ball, cuboid or capsule collider, or \
                 the RigidBody's mass properties"
                    .into(),
            )),
            (true, true) => Err(invalid(
                "sim.body_invalid",
                entity,
                "reason",
                "a dynamic body takes its mass from the RigidBody or from the collider's density, \
                 not both (numeric.md 5)"
                    .into(),
            )),
            _ => Ok(()),
        }
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> EntityId {
        EntityId::FIRST
    }

    fn hull() -> Shape {
        Shape::ConvexHull {
            points: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
        }
    }

    #[test]
    fn the_mass_rule() {
        let dynamic = RigidBody::dynamic();
        let stated = RigidBody::dynamic().with_mass(MassProps::solid_box(1.0, [0.5; 3], [0.0; 3]));
        // A dynamic hull, mesh or compound with a density: refused.
        let e = validate(
            id(),
            Some(&dynamic),
            &Collider::new(hull()).with_density(1.0),
        )
        .unwrap_err();
        assert_eq!(e.code, "sim.collider_density");
        assert_eq!(e.detail["collider"], json!("ConvexHull"));
        let compound = Shape::Compound {
            parts: vec![Part {
                position: [0.0; 3],
                rotation: geom::IDENTITY,
                shape: PartShape::Ball { radius: 1.0 },
            }],
        };
        let e = validate(
            id(),
            Some(&stated),
            &Collider::new(compound.clone()).with_density(2.0),
        );
        assert_eq!(e.unwrap_err().code, "sim.collider_density");
        // Stated mass properties instead: accepted.
        assert!(validate(id(), Some(&stated), &Collider::new(hull())).is_ok());
        assert!(validate(id(), Some(&stated), &Collider::new(compound)).is_ok());
        // A fixed hull with a density too: Rapier computes its mass properties all the same.
        let fixed = validate(
            id(),
            Some(&RigidBody::fixed()),
            &Collider::new(hull()).with_density(1.0),
        );
        assert_eq!(fixed.unwrap_err().code, "sim.collider_density");
        assert!(validate(id(), None, &Collider::new(hull())).is_ok());
        // A cuboid takes a density, but not with stated mass properties too, and needs one or the other.
        let cube = Collider::new(Shape::Cuboid {
            half_extents: [0.3; 3],
        });
        assert!(validate(id(), Some(&dynamic), &cube.clone().with_density(500.0)).is_ok());
        assert_eq!(
            validate(id(), Some(&stated), &cube.clone().with_density(500.0))
                .unwrap_err()
                .code,
            "sim.body_invalid"
        );
        assert_eq!(
            validate(id(), Some(&dynamic), &cube).unwrap_err().code,
            "sim.body_invalid"
        );
        assert_eq!(
            validate(id(), Some(&dynamic), &Collider::new(hull()))
                .unwrap_err()
                .code,
            "sim.body_invalid"
        );
        // Degenerate shapes.
        let flat = Collider::new(Shape::Ball { radius: 0.0 }).with_density(1.0);
        assert_eq!(
            validate(id(), Some(&dynamic), &flat).unwrap_err().code,
            "sim.collider_invalid"
        );
        let mesh = Shape::TriMesh {
            vertices: vec![[0.0; 3]; 3],
            indices: vec![[0, 1, 3]],
        };
        assert_eq!(
            validate(id(), None, &Collider::new(mesh)).unwrap_err().code,
            "sim.collider_invalid"
        );
    }

    /// Values finite and positive in `f64` that the solver's `f32` makes 0 or infinite are
    /// refused: 1e39 overflows it, 1e-50 and 1e-300 round to 0.
    #[test]
    fn values_are_judged_as_the_solver_receives_them() {
        let dynamic = RigidBody::dynamic();
        let ball = |r: f64| Collider::new(Shape::Ball { radius: r }).with_density(1000.0);
        let code = |b: &RigidBody, c: &Collider| validate(id(), Some(b), c).unwrap_err().code;
        for r in [1e39, 1e-50, 1e-300] {
            assert_eq!(
                code(&dynamic, &ball(r)),
                "sim.collider_invalid",
                "radius {r}"
            );
        }
        // A radius of 1e-30 is one `f32` holds; the mass its density gives rounds to 0, which the
        // step refuses when it builds the body (`solver`, tests/bodies.rs).
        assert!(validate(id(), Some(&dynamic), &ball(1e-30)).is_ok());
        let ground = Collider::new(Shape::Cuboid {
            half_extents: [1e39, 0.5, 1e39],
        });
        assert_eq!(code(&RigidBody::fixed(), &ground), "sim.collider_invalid");
        let capsule = Collider::new(Shape::Capsule {
            half_height: 1e39,
            radius: 0.5,
        });
        assert_eq!(code(&RigidBody::fixed(), &capsule), "sim.collider_invalid");
        let far = Shape::ConvexHull {
            points: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1e39]],
        };
        assert_eq!(
            code(&RigidBody::fixed(), &Collider::new(far)),
            "sim.collider_invalid"
        );
        let part = |position: V3, rotation: Q4| Shape::Compound {
            parts: vec![Part {
                position,
                rotation,
                shape: PartShape::Ball { radius: 1.0 },
            }],
        };
        for shape in [
            part([1e39, 0.0, 0.0], geom::IDENTITY),
            part([0.0; 3], [1e200, 0.0, 0.0, 0.0]),
            part([0.0; 3], [0.0; 4]),
        ] {
            assert_eq!(
                code(&RigidBody::fixed(), &Collider::new(shape)),
                "sim.collider_invalid"
            );
        }
        for d in [1e39, 1e-50] {
            let c = Collider::new(Shape::Ball { radius: 1.0 }).with_density(d);
            assert_eq!(code(&dynamic, &c), "sim.collider_invalid", "density {d}");
        }
        let slippery = ball(1.0).with_friction(1e39, 0.0);
        assert_eq!(code(&dynamic, &slippery), "sim.collider_invalid");
        // Stated mass properties: a mass or an inertia of 1e-300 or 1e39, a centre at 1e39.
        let hull = Collider::new(hull());
        for (mass, inertia, center) in [
            (1e-300, 1.0, 0.0),
            (1.0, 1e-300, 0.0),
            (1e39, 1.0, 0.0),
            (1.0, 1.0, 1e39),
        ] {
            let stated = RigidBody::dynamic().with_mass(MassProps {
                mass,
                center: [center, 0.0, 0.0],
                inertia: [inertia; 3],
            });
            assert_eq!(code(&stated, &hull), "sim.body_invalid", "{mass} {inertia}");
        }
        let damped = RigidBody {
            linear_damping: 1e39,
            ..RigidBody::dynamic()
        };
        assert_eq!(code(&damped, &ball(1.0)), "sim.body_invalid");
    }
}
