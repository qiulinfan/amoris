//! `RigidBody`, `Collider` and `ExternalForce` for scripts (script-host.md 7.2): the first engine
//! components whose Rust forms are not flat. Each schema is a flat projection of its component: an
//! optional number is a slot whose 0 is none (`density`, `mass_kg`), and the collider's tagged
//! shape is its tag as the enum `kind` with one field per size of the kinds a script builds (a
//! ball, a cuboid, a capsule). A hull, a mesh, a compound or a heightfield reads as its kind with
//! sizes of 0.
//!
//! The slot functions write in place and by part: a part of the component is replaced only when
//! its slots differ bit for bit from what the component reads, so `write(c, read(c))` leaves every
//! component as it was, and a write of friction neither loses nor rebuilds a mesh. What the flat
//! form cannot say is refused at the script's calling line by the slot rules below
//! (`script.write_not_held`), which also settle a staged value to what the component will read
//! back. Whether a body can be built (sizes above 0, the mass rule of numeric.md 5) stays with
//! `physics.step`, which judges a scene's and `world.edit`'s bodies the same way.
//!
//! No script field is named `shape` or `mass`: the write trace reads a JSON path's change from the
//! slots of the script field of the same name (writes.rs, `Slots::resolve`), so a script field
//! named like a field of the JSON form must hold all of it.

use amoris_contract::Problem;
use amoris_physics::{BodyKind, Collider, ExternalForce, MassProps, RigidBody, Shape};
use amoris_script::{EngineFns, SlotRefusal, register_engine, register_engine_rule};
use amoris_sim::Persisted;
use amoris_sim::registry::{ComponentSchema, FieldType, FieldValue, ProjectValues};
use bevy_ecs::prelude::World;

use super::{nums, schema, v3};

/// `RigidBody.kind`'s variants in slot order: `BodyKind`'s JSON names.
const BODY_KINDS: [&str; 2] = ["Dynamic", "Fixed"];

/// `Collider.kind`'s variants in slot order: `Shape`'s JSON tags. A script builds the first
/// [`BUILT`] of them.
const SHAPE_KINDS: [&str; 7] = [
    "Ball",
    "Cuboid",
    "Capsule",
    "ConvexHull",
    "TriMesh",
    "Compound",
    "HeightField",
];

/// How many of [`SHAPE_KINDS`] a script builds: the ball, the cuboid and the capsule.
const BUILT: usize = 3;

// Slots of `RigidBody`.
const MASS: usize = 1;
const CENTER: usize = 2;
const INERTIA: usize = 5;
/// One past the stated mass's slots (`mass_kg`, `center_of_mass`, `inertia`).
const MASS_END: usize = 8;

// Slots of `Collider`.
const RADIUS: usize = 1;
const HALF_HEIGHT: usize = 2;
const HALF_EXTENTS: usize = 3;
/// One past the shape's slots (`kind` and the sizes).
const SHAPE_END: usize = 6;
const DENSITY: usize = 6;

fn body_kind_slot(kind: BodyKind) -> u32 {
    match kind {
        BodyKind::Dynamic => 0,
        BodyKind::Fixed => 1,
    }
}

fn same_bits(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

pub(super) fn rigid_body_schema() -> Result<ComponentSchema, Problem> {
    let d = RigidBody::default();
    let f = |name, value: f64, doc| (name, FieldType::F64, FieldValue::F64(value), doc);
    let v = |name, doc| (name, FieldType::Vec3, FieldValue::Vec3([0.0; 3]), doc);
    schema(
        "RigidBody",
        RigidBody::VERSION,
        "A rigid body: with a Transform and a Collider on its entity, the solver moves it. A \
         Collider without a RigidBody is fixed. Give a moving body a Velocity (without one a \
         change to it or to its Collider restarts it at rest) and an ExternalForce to push it. A \
         dynamic body takes its mass from one place: its Collider's density, or mass_kg with \
         inertia here.",
        vec![
            (
                "kind",
                FieldType::Enum(BODY_KINDS.map(Box::from).into()),
                FieldValue::Enum(body_kind_slot(d.kind)),
                "Dynamic: moved by forces, gravity and contacts. Fixed: never moves.",
            ),
            f(
                "mass_kg",
                0.0,
                "The stated mass, kg; 0: none, and the Collider's density gives the mass.",
            ),
            v(
                "center_of_mass",
                "The centre of mass in the body's frame, metres; held only with mass_kg.",
            ),
            v(
                "inertia",
                "Principal moments of inertia about the body's axes through the centre of mass, \
                 kg m^2, each above 0; held only with mass_kg.",
            ),
            f(
                "linear_damping",
                d.linear_damping,
                "Linear velocity damping, per second.",
            ),
            f(
                "angular_damping",
                d.angular_damping,
                "Angular velocity damping, per second.",
            ),
            f(
                "gravity_scale",
                d.gravity_scale,
                "Gravity's multiplier on this body.",
            ),
        ],
    )
}

pub(super) fn collider_schema() -> Result<ComponentSchema, Problem> {
    let d = Collider::new(Shape::Ball { radius: 0.0 });
    let f = |name, value: f64, doc| (name, FieldType::F64, FieldValue::F64(value), doc);
    schema(
        "Collider",
        Collider::VERSION,
        "The collider of the body on its entity. A script builds a Ball, a Cuboid or a Capsule; \
         a ConvexHull, a TriMesh, a Compound and a HeightField come from the scene, and a script \
         reads their kind (sizes 0) and changes their friction and restitution. The scene's \
         shape: {\"Ball\": {\"radius\": 0.2}} is kind: \"Ball\", radius: 0.2 here.",
        vec![
            (
                "kind",
                FieldType::Enum(SHAPE_KINDS.map(Box::from).into()),
                FieldValue::Enum(0),
                "The shape. A script writes Ball, Cuboid or Capsule; the other kinds are read only.",
            ),
            f(
                "radius",
                0.0,
                "A Ball's or a Capsule's radius, metres; 0 on the other kinds.",
            ),
            f(
                "half_height",
                0.0,
                "Half the length of a Capsule's segment along +y, metres; 0 on the other kinds.",
            ),
            (
                "half_extents",
                FieldType::Vec3,
                FieldValue::Vec3([0.0; 3]),
                "A Cuboid's half extents, metres; 0 on the other kinds.",
            ),
            f(
                "density",
                0.0,
                "kg/m^3, giving the body its mass, on a Ball, a Cuboid or a Capsule; 0: none, and \
                 the RigidBody states the mass.",
            ),
            f("friction", d.friction, "Coulomb friction, at least 0."),
            f("restitution", d.restitution, "Bounciness, at least 0."),
        ],
    )
}

pub(super) fn external_force_schema() -> Result<ComponentSchema, Problem> {
    let v = |name, doc| (name, FieldType::Vec3, FieldValue::Vec3([0.0; 3]), doc);
    schema(
        "ExternalForce",
        ExternalForce::VERSION,
        "The force and torque on the body over this tick's step, in the world frame. The step \
         applies it and sets it back to zero, so add to it in every tick you push; an impulse J \
         is the force J / ctx.dt for one tick.",
        vec![
            v("force", "Newtons."),
            v("torque", "Newton metres, about the centre of mass."),
        ],
    )
}

/// The stated mass's slots: `mass_kg`, `center_of_mass`, `inertia`; zeros without one.
fn mass_slots(mass: Option<MassProps>) -> [f64; 7] {
    match mass {
        Some(m) => [
            m.mass,
            m.center[0],
            m.center[1],
            m.center[2],
            m.inertia[0],
            m.inertia[1],
            m.inertia[2],
        ],
        None => [0.0; 7],
    }
}

fn rigid_body_read(b: &RigidBody) -> ProjectValues {
    let m = mass_slots(b.mass);
    nums(&[
        f64::from(body_kind_slot(b.kind)),
        m[0],
        m[1],
        m[2],
        m[3],
        m[4],
        m[5],
        m[6],
        b.linear_damping,
        b.angular_damping,
        b.gravity_scale,
    ])
}

/// In place: the stated mass is replaced only when its slots changed, so a damping write keeps it
/// as it is, an invalid one a scene stated included.
fn rigid_body_write(b: &mut RigidBody, v: &ProjectValues) {
    let n = &v.nums;
    b.kind = if n[0] == 1.0 {
        BodyKind::Fixed
    } else {
        BodyKind::Dynamic
    };
    if !same_bits(&n[MASS..MASS_END], &mass_slots(b.mass)) {
        b.mass = (n[MASS] != 0.0).then(|| MassProps {
            mass: n[MASS],
            center: v3(n, CENTER),
            inertia: v3(n, INERTIA),
        });
    }
    b.linear_damping = n[8];
    b.angular_damping = n[9];
    b.gravity_scale = n[10];
}

fn rigid_body_fns() -> EngineFns<RigidBody> {
    EngineFns {
        read: rigid_body_read,
        write: rigid_body_write,
        make: |v| {
            let mut b = RigidBody::default();
            rigid_body_write(&mut b, v);
            b
        },
    }
}

/// The shape's slots: its kind, then `radius`, `half_height` and `half_extents`, 0 where the kind
/// has no such size. Clones nothing: a query reads every row.
fn shape_slots(shape: &Shape) -> [f64; SHAPE_END] {
    match shape {
        Shape::Ball { radius } => [0.0, *radius, 0.0, 0.0, 0.0, 0.0],
        Shape::Cuboid { half_extents: h } => [1.0, 0.0, 0.0, h[0], h[1], h[2]],
        Shape::Capsule {
            half_height,
            radius,
        } => [2.0, *radius, *half_height, 0.0, 0.0, 0.0],
        Shape::ConvexHull { .. } => [3.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        Shape::TriMesh { .. } => [4.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        Shape::Compound { .. } => [5.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        Shape::HeightField { .. } => [6.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    }
}

fn collider_read(c: &Collider) -> ProjectValues {
    let s = shape_slots(&c.shape);
    nums(&[
        s[0],
        s[1],
        s[2],
        s[3],
        s[4],
        s[5],
        c.density.unwrap_or(0.0),
        c.friction,
        c.restitution,
    ])
}

/// In place: the shape is replaced only when its slots changed, and only by a ball, a cuboid or a
/// capsule built from that kind's own sizes; a hull, a mesh, a compound or a heightfield is never
/// made here, so one a scene authored stays whatever else the slots say. The density likewise.
fn collider_write(c: &mut Collider, v: &ProjectValues) {
    let n = &v.nums;
    if !same_bits(&n[..SHAPE_END], &shape_slots(&c.shape)) {
        if n[0] == 0.0 {
            c.shape = Shape::Ball { radius: n[RADIUS] };
        } else if n[0] == 1.0 {
            c.shape = Shape::Cuboid {
                half_extents: v3(n, HALF_EXTENTS),
            };
        } else if n[0] == 2.0 {
            c.shape = Shape::Capsule {
                half_height: n[HALF_HEIGHT],
                radius: n[RADIUS],
            };
        }
    }
    if n[DENSITY].to_bits() != c.density.unwrap_or(0.0).to_bits() {
        c.density = (n[DENSITY] != 0.0).then_some(n[DENSITY]);
    }
    c.friction = n[7];
    c.restitution = n[8];
}

fn collider_fns() -> EngineFns<Collider> {
    EngineFns {
        read: collider_read,
        write: collider_write,
        make: |v| {
            let mut c = Collider::new(Shape::Ball { radius: 0.0 });
            collider_write(&mut c, v);
            c
        },
    }
}

fn external_force_fns() -> EngineFns<ExternalForce> {
    EngineFns {
        read: |c| {
            let (f, t) = (c.force, c.torque);
            nums(&[f[0], f[1], f[2], t[0], t[1], t[2]])
        },
        write: |c, v| {
            c.force = v3(&v.nums, 0);
            c.torque = v3(&v.nums, 3);
        },
        make: |v| ExternalForce {
            force: v3(&v.nums, 0),
            torque: v3(&v.nums, 3),
        },
    }
}

/// Whether slot `i` is what the entity already holds, bit for bit; never for an insert or a spawn.
fn same(before: Option<&ProjectValues>, after: &ProjectValues, i: usize) -> bool {
    before.is_some_and(|b| b.nums[i].to_bits() == after.nums[i].to_bits())
}

/// A validated enum slot as an index.
fn index(x: f64) -> usize {
    amoris_sim::num::to_u32(x)
        .ok()
        .and_then(|i| usize::try_from(i).ok())
        .unwrap_or(0)
}

/// `Collider`'s slot rule: a script cannot make a hull, a mesh, a compound or a heightfield (a
/// kind the collider already has stays), and a size belongs to the kinds that take it. A size the
/// kind does not take reads 0, so one left by a change of kind is settled to 0, as is a density of
/// `-0`, which is none.
fn collider_rule(
    before: Option<&ProjectValues>,
    after: &mut ProjectValues,
) -> Result<(), SlotRefusal> {
    let kind = index(after.nums[0]).min(SHAPE_KINDS.len() - 1);
    let name = SHAPE_KINDS[kind];
    if kind >= BUILT && !same(before, after, 0) {
        return Err(SlotRefusal {
            field: "kind",
            message: format!(
                "Collider.kind cannot become {name}: a script builds Ball, Cuboid and Capsule \
                 colliders; a hull, mesh, compound or heightfield comes from the scene."
            ),
        });
    }
    // Which of radius, half_height and half_extents the kind takes, and how to say so.
    let (takes, instead) = match kind {
        0 => ([true, false, false], "a Ball takes radius"),
        1 => ([false, false, true], "a Cuboid takes half_extents"),
        2 => (
            [true, true, false],
            "a Capsule takes radius and half_height",
        ),
        _ => ([false; 3], ""),
    };
    let sizes = [
        ("radius", "a Ball's or a Capsule's", RADIUS..HALF_HEIGHT),
        ("half_height", "a Capsule's", HALF_HEIGHT..HALF_EXTENTS),
        ("half_extents", "a Cuboid's", HALF_EXTENTS..SHAPE_END),
    ];
    for ((field, whose, slots), taken) in sizes.into_iter().zip(takes) {
        if taken {
            continue;
        }
        if slots
            .clone()
            .any(|i| after.nums[i] != 0.0 && !same(before, after, i))
        {
            let instead = if kind < BUILT {
                instead.to_owned()
            } else {
                format!("a {name}'s shape comes from the scene and a script cannot resize it")
            };
            return Err(SlotRefusal {
                field,
                message: format!("Collider.{field} is {whose} size; {instead}."),
            });
        }
        for i in slots {
            after.nums[i] = 0.0;
        }
    }
    if after.nums[DENSITY] == 0.0 && !same(before, after, DENSITY) {
        after.nums[DENSITY] = 0.0;
    }
    Ok(())
}

/// `RigidBody`'s slot rule: a centre of mass and inertias are held only with a stated mass. Once a
/// write leaves no stated mass they read 0, so those a cleared `mass_kg` left are settled to 0.
fn rigid_body_rule(
    before: Option<&ProjectValues>,
    after: &mut ProjectValues,
) -> Result<(), SlotRefusal> {
    if after.nums[MASS] != 0.0 {
        return Ok(());
    }
    for (field, mut slots) in [
        ("center_of_mass", CENTER..INERTIA),
        ("inertia", INERTIA..MASS_END),
    ] {
        if slots.any(|i| after.nums[i] != 0.0 && !same(before, after, i)) {
            return Err(SlotRefusal {
                field,
                message: format!(
                    "RigidBody.{field} belongs to a stated mass: give mass_kg with it, or leave \
                     it out when the collider's density gives the mass."
                ),
            });
        }
    }
    // As `rigid_body_write` does: a change among these slots that leaves mass_kg 0 leaves no
    // stated mass, and none changed leaves what was there (a stated mass of 0, which only a scene
    // or world.edit can write, included).
    if (MASS..MASS_END).any(|i| !same(before, after, i)) {
        for i in MASS..MASS_END {
            after.nums[i] = 0.0;
        }
    }
    Ok(())
}

/// Registers the slot functions and the slot rules; the schemas are in the registry already.
pub(super) fn register(world: &mut World) -> Result<(), Problem> {
    register_engine::<RigidBody>(world, rigid_body_fns())?;
    register_engine::<Collider>(world, collider_fns())?;
    register_engine::<ExternalForce>(world, external_force_fns())?;
    register_engine_rule::<RigidBody>(world, rigid_body_rule)?;
    register_engine_rule::<Collider>(world, collider_rule)
}

#[cfg(test)]
mod tests {
    use amoris_physics::{Part, PartShape};
    use serde_json::{Value, json};

    use super::*;

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

    fn mesh() -> Shape {
        Shape::TriMesh {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            indices: vec![[0, 2, 1]],
        }
    }

    /// One shape of every kind, in slot order.
    fn shapes() -> Vec<Shape> {
        vec![
            Shape::Ball { radius: 0.2 },
            Shape::Cuboid {
                half_extents: [1.0, 2.0, 3.0],
            },
            Shape::Capsule {
                half_height: 0.7,
                radius: 0.3,
            },
            hull(),
            mesh(),
            Shape::Compound {
                parts: vec![Part {
                    position: [0.0, 1.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    shape: PartShape::Ball { radius: 0.5 },
                }],
            },
            Shape::HeightField {
                size: [1, 1],
                cell_m: 2.0,
                heights: vec![0.0, 1.0, 0.5, 0.25],
            },
        ]
    }

    fn colliders() -> Vec<Collider> {
        let mut out = Vec::new();
        for shape in shapes() {
            out.push(Collider::new(shape.clone()));
            out.push(
                Collider::new(shape.clone())
                    .with_density(750.0)
                    .with_friction(0.9, 0.4),
            );
            // A density of 0, which only a scene or world.edit can state.
            out.push(Collider::new(shape).with_density(0.0));
        }
        out
    }

    fn bodies() -> Vec<RigidBody> {
        let stated = MassProps {
            mass: 2.0,
            center: [0.1, 0.2, 0.3],
            inertia: [0.4, 0.5, 0.6],
        };
        vec![
            RigidBody::default(),
            RigidBody::fixed(),
            RigidBody {
                linear_damping: 0.5,
                angular_damping: 0.25,
                gravity_scale: 0.0,
                ..RigidBody::dynamic().with_mass(stated)
            },
            // A stated mass of 0, which only a scene or world.edit can state.
            RigidBody::dynamic().with_mass(MassProps {
                mass: 0.0,
                ..stated
            }),
        ]
    }

    fn with(v: &ProjectValues, changes: &[(usize, f64)]) -> ProjectValues {
        let mut out = v.clone();
        for (slot, x) in changes {
            out.nums[*slot] = *x;
        }
        out
    }

    fn slot_of(schema: &ComponentSchema, field: &str) -> usize {
        let f = schema.fields.iter().find(|f| &*f.name == field);
        usize::from(f.unwrap_or_else(|| panic!("no field {field}")).first_slot)
    }

    #[test]
    fn the_slots_are_the_schemas() {
        let (rb, co, ef) = (
            rigid_body_schema().unwrap(),
            collider_schema().unwrap(),
            external_force_schema().unwrap(),
        );
        assert_eq!((rb.slots(), co.slots(), ef.slots()), (11, 9, 6));
        for b in bodies() {
            assert_eq!(rigid_body_read(&b).nums.len(), 11);
        }
        for c in colliders() {
            assert_eq!(collider_read(&c).nums.len(), 9);
        }
        let force = (external_force_fns().read)(&ExternalForce::default());
        assert_eq!(force.nums.len(), 6);
        // The named slots are the fields'.
        assert_eq!(
            [
                slot_of(&rb, "mass_kg"),
                slot_of(&rb, "center_of_mass"),
                slot_of(&rb, "inertia"),
                slot_of(&rb, "linear_damping"),
            ],
            [MASS, CENTER, INERTIA, MASS_END]
        );
        assert_eq!(
            [
                slot_of(&co, "radius"),
                slot_of(&co, "half_height"),
                slot_of(&co, "half_extents"),
                slot_of(&co, "density"),
            ],
            [RADIUS, HALF_HEIGHT, HALF_EXTENTS, DENSITY]
        );
        assert_eq!(SHAPE_END, DENSITY);
        // The defaults are the components': what `insert(e, "RigidBody")` makes.
        let made = (rigid_body_fns().make)(&amoris_script::access::defaults(&rb));
        assert_eq!(made, RigidBody::default());
        let made = (collider_fns().make)(&amoris_script::access::defaults(&co));
        assert_eq!(made, Collider::new(Shape::Ball { radius: 0.0 }));
    }

    #[test]
    fn writing_back_what_was_read_changes_nothing() {
        for c in colliders() {
            let mut out = c.clone();
            collider_write(&mut out, &collider_read(&c));
            assert_eq!(out, c);
        }
        for b in bodies() {
            let mut out = b;
            rigid_body_write(&mut out, &rigid_body_read(&b));
            assert_eq!(out, b);
        }
        let f = ExternalForce {
            force: [1.0, -2.0, 3.0],
            torque: [0.5, 0.25, -0.125],
        };
        let fns = external_force_fns();
        assert_eq!((fns.make)(&(fns.read)(&f)), f);
        let mut out = ExternalForce::default();
        (fns.write)(&mut out, &(fns.read)(&f));
        assert_eq!(out, f);
    }

    #[test]
    fn a_write_that_leaves_the_shape_slots_alone_keeps_an_authored_shape() {
        for shape in shapes() {
            let c = Collider::new(shape.clone());
            let mut out = c.clone();
            let v = with(&collider_read(&c), &[(7, 0.9), (8, 0.3)]);
            collider_write(&mut out, &v);
            assert_eq!(out.shape, shape);
            assert_eq!((out.friction, out.restitution), (0.9, 0.3));
            assert_eq!(out.density, None);
        }
        // A size written over a mesh without its kind changes nothing (the rule refuses it before
        // it comes to this), and a density is the step's to refuse.
        let mut out = Collider::new(mesh());
        let v = with(&collider_read(&out), &[(RADIUS, 2.0), (DENSITY, 100.0)]);
        collider_write(&mut out, &v);
        assert_eq!(out.shape, mesh());
        assert_eq!(out.density, Some(100.0));
        // A ball asked for over a mesh replaces it.
        let v = with(&collider_read(&out), &[(0, 0.0), (RADIUS, 2.0)]);
        collider_write(&mut out, &v);
        assert_eq!(out.shape, Shape::Ball { radius: 2.0 });
    }

    #[test]
    fn made_from_defaults_and_a_patch() {
        let co = amoris_script::access::defaults(&collider_schema().unwrap());
        let make = collider_fns().make;
        let ball = make(&with(&co, &[(RADIUS, 0.2), (DENSITY, 500.0)]));
        assert_eq!(
            ball,
            Collider::new(Shape::Ball { radius: 0.2 }).with_density(500.0)
        );
        let cuboid = make(&with(
            &co,
            &[(0, 1.0), (HALF_EXTENTS, 10.0), (4, 0.5), (5, 10.0)],
        ));
        assert_eq!(
            cuboid,
            Collider::new(Shape::Cuboid {
                half_extents: [10.0, 0.5, 10.0]
            })
        );
        assert_eq!(cuboid.density, None, "a density of 0 is none");
        let capsule = make(&with(&co, &[(0, 2.0), (RADIUS, 0.3), (HALF_HEIGHT, 0.6)]));
        assert_eq!(
            capsule.shape,
            Shape::Capsule {
                half_height: 0.6,
                radius: 0.3
            }
        );
        // Each kind takes its own sizes only.
        let stale = make(&with(&co, &[(0, 1.0), (RADIUS, 0.2), (HALF_EXTENTS, 1.0)]));
        assert_eq!(
            stale.shape,
            Shape::Cuboid {
                half_extents: [1.0, 0.0, 0.0]
            }
        );

        let rb = amoris_script::access::defaults(&rigid_body_schema().unwrap());
        let make = rigid_body_fns().make;
        assert_eq!(make(&rb), RigidBody::default());
        assert_eq!(make(&with(&rb, &[(0, 1.0)])), RigidBody::fixed());
        let stated = make(&with(
            &rb,
            &[
                (MASS, 2.0),
                (CENTER + 1, 0.1),
                (INERTIA, 0.4),
                (INERTIA + 1, 0.4),
                (INERTIA + 2, 0.4),
            ],
        ));
        let mass = MassProps {
            mass: 2.0,
            center: [0.0, 0.1, 0.0],
            inertia: [0.4; 3],
        };
        assert_eq!(stated, RigidBody::dynamic().with_mass(mass));
        // A damping write keeps the stated mass; clearing mass_kg drops it.
        let mut out = stated;
        rigid_body_write(&mut out, &with(&rigid_body_read(&stated), &[(8, 0.5)]));
        assert_eq!(out.mass, Some(mass));
        assert_eq!(out.linear_damping, 0.5);
        let cleared = with(&rigid_body_read(&out), &[(MASS, 0.0)]);
        rigid_body_write(&mut out, &cleared);
        assert_eq!(out.mass, None);
    }

    /// An accepted write: the rule's settled values are what the component reads after the write
    /// (`set` and columns), and what a component made from them reads (`insert` and `spawn`).
    fn settles<C: Clone>(
        fns: &EngineFns<C>,
        rule: amoris_script::SlotRule,
        c: &C,
        changes: &[(usize, f64)],
    ) -> ProjectValues {
        let before = (fns.read)(c);
        let written = with(&before, changes);
        let mut settled = written.clone();
        rule(Some(&before), &mut settled)
            .unwrap_or_else(|r| panic!("{changes:?} refused: {}", r.message));
        let mut out = c.clone();
        (fns.write)(&mut out, &written);
        assert_eq!((fns.read)(&out), settled, "{changes:?}: written as given");
        let mut out = c.clone();
        (fns.write)(&mut out, &settled);
        assert_eq!((fns.read)(&out), settled, "{changes:?}: written as settled");
        settled
    }

    fn refused<C>(
        fns: &EngineFns<C>,
        rule: amoris_script::SlotRule,
        c: Option<&C>,
        schema: &ComponentSchema,
        changes: &[(usize, f64)],
    ) -> SlotRefusal {
        let before = c.map(|c| (fns.read)(c));
        let base = before
            .clone()
            .unwrap_or_else(|| amoris_script::access::defaults(schema));
        let mut after = with(&base, changes);
        match rule(before.as_ref(), &mut after) {
            Err(r) => r,
            Ok(()) => panic!("{changes:?} accepted"),
        }
    }

    #[test]
    fn the_collider_rule() {
        let fns = collider_fns();
        let schema = collider_schema().unwrap();
        let ball = Collider::new(Shape::Ball { radius: 0.2 }).with_density(500.0);
        // What a Ball takes and what it does not.
        settles(&fns, collider_rule, &ball, &[(RADIUS, 0.5), (7, 0.9)]);
        settles(&fns, collider_rule, &ball, &[(DENSITY, 0.0)]);
        let none = settles(&fns, collider_rule, &ball, &[(DENSITY, -0.0)]);
        assert_eq!(none.nums[DENSITY].to_bits(), 0.0f64.to_bits());
        let r = refused(&fns, collider_rule, Some(&ball), &schema, &[(4, 1.0)]);
        assert_eq!(r.field, "half_extents");
        assert_eq!(
            r.message,
            "Collider.half_extents is a Cuboid's size; a Ball takes radius."
        );
        let r = refused(
            &fns,
            collider_rule,
            Some(&ball),
            &schema,
            &[(HALF_HEIGHT, 1.0)],
        );
        assert_eq!(r.field, "half_height");
        // A change of kind: the new kind's sizes are taken and the old one's settle to 0.
        let cuboid = settles(
            &fns,
            collider_rule,
            &ball,
            &[(0, 1.0), (3, 1.0), (4, 2.0), (5, 3.0)],
        );
        assert_eq!(cuboid.nums[..SHAPE_END], [1.0, 0.0, 0.0, 1.0, 2.0, 3.0]);
        let capsule = settles(&fns, collider_rule, &ball, &[(0, 2.0), (HALF_HEIGHT, 0.5)]);
        assert_eq!(capsule.nums[..SHAPE_END], [2.0, 0.2, 0.5, 0.0, 0.0, 0.0]);
        let r = refused(
            &fns,
            collider_rule,
            Some(&ball),
            &schema,
            &[(0, 1.0), (RADIUS, 0.3)],
        );
        assert_eq!(r.field, "radius");
        assert_eq!(
            r.message,
            "Collider.radius is a Ball's or a Capsule's size; a Cuboid takes half_extents."
        );
        // The four kinds a scene authors: kept, never made, never resized.
        for shape in shapes().into_iter().skip(BUILT) {
            let kind = shape_slots(&shape)[0];
            let authored = Collider::new(shape);
            settles(&fns, collider_rule, &authored, &[]);
            settles(&fns, collider_rule, &authored, &[(7, 0.9), (8, 0.2)]);
            // A density on it is the step's to refuse (sim.collider_density), not the rule's.
            settles(&fns, collider_rule, &authored, &[(DENSITY, 100.0)]);
            let r = refused(
                &fns,
                collider_rule,
                Some(&authored),
                &schema,
                &[(RADIUS, 1.0)],
            );
            assert_eq!(r.field, "radius");
            assert!(r.message.contains("comes from the scene"), "{}", r.message);
            let r = refused(&fns, collider_rule, Some(&ball), &schema, &[(0, kind)]);
            assert_eq!(r.field, "kind");
            let r = refused(&fns, collider_rule, None, &schema, &[(0, kind)]);
            assert_eq!(r.field, "kind");
            // A ball asked for over it replaces it.
            let made = settles(&fns, collider_rule, &authored, &[(0, 0.0), (RADIUS, 1.0)]);
            assert_eq!(made.nums[..SHAPE_END], [0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        }
        let r = refused(
            &fns,
            collider_rule,
            None,
            &schema,
            &[(0, 4.0), (RADIUS, 1.0)],
        );
        assert_eq!(
            r.message,
            "Collider.kind cannot become TriMesh: a script builds Ball, Cuboid and Capsule \
             colliders; a hull, mesh, compound or heightfield comes from the scene."
        );
        // An insert or a spawn: what the rule settles is what the made component reads.
        for changes in [
            vec![(RADIUS, 0.2), (DENSITY, 500.0)],
            vec![(0, 1.0), (3, 1.0), (4, 1.0), (5, 1.0), (DENSITY, -0.0)],
            vec![(0, 2.0), (RADIUS, 0.3), (HALF_HEIGHT, 0.6)],
            vec![],
        ] {
            let mut v = with(&amoris_script::access::defaults(&schema), &changes);
            collider_rule(None, &mut v).unwrap_or_else(|r| panic!("{}", r.message));
            assert_eq!(collider_read(&(fns.make)(&v)), v, "{changes:?}");
        }
        let r = refused(&fns, collider_rule, None, &schema, &[(3, 1.0)]);
        assert_eq!(r.field, "half_extents");
    }

    #[test]
    fn the_rigid_body_rule() {
        let fns = rigid_body_fns();
        let schema = rigid_body_schema().unwrap();
        let plain = RigidBody::default();
        let stated = bodies()[2];
        settles(&fns, rigid_body_rule, &plain, &[(8, 0.5), (10, 0.0)]);
        settles(&fns, rigid_body_rule, &plain, &[(0, 1.0)]);
        settles(&fns, rigid_body_rule, &plain, &[(MASS, -0.0)]);
        // A mass with its centre and inertia; a change of either under a stated mass.
        settles(
            &fns,
            rigid_body_rule,
            &plain,
            &[(MASS, 2.0), (INERTIA, 0.4), (6, 0.4), (7, 0.4)],
        );
        settles(&fns, rigid_body_rule, &stated, &[(CENTER, 0.5), (7, 0.9)]);
        settles(&fns, rigid_body_rule, &stated, &[(9, 0.1)]);
        // Clearing the mass leaves none: its centre and inertia settle to 0.
        let cleared = settles(&fns, rigid_body_rule, &stated, &[(MASS, 0.0)]);
        assert_eq!(cleared.nums[MASS..MASS_END], [0.0; 7]);
        // Without a mass neither is held.
        let r = refused(&fns, rigid_body_rule, Some(&plain), &schema, &[(6, 0.4)]);
        assert_eq!(r.field, "inertia");
        assert_eq!(
            r.message,
            "RigidBody.inertia belongs to a stated mass: give mass_kg with it, or leave it out \
             when the collider's density gives the mass."
        );
        let r = refused(
            &fns,
            rigid_body_rule,
            Some(&plain),
            &schema,
            &[(CENTER, 0.1)],
        );
        assert_eq!(r.field, "center_of_mass");
        let r = refused(
            &fns,
            rigid_body_rule,
            Some(&stated),
            &schema,
            &[(MASS, 0.0), (INERTIA, 9.0)],
        );
        assert_eq!(r.field, "inertia");
        let r = refused(&fns, rigid_body_rule, None, &schema, &[(INERTIA, 0.4)]);
        assert_eq!(r.field, "inertia");
        // A scene's stated mass of 0 stays as it is under a write of something else.
        let odd = bodies()[3];
        let kept = settles(&fns, rigid_body_rule, &odd, &[(8, 0.5)]);
        assert_eq!(kept.nums[INERTIA], 0.4);
        // An insert or a spawn.
        for changes in [
            vec![],
            vec![(0, 1.0), (10, 0.0)],
            vec![(MASS, 2.0), (INERTIA, 0.4), (6, 0.4), (7, 0.4)],
            vec![(MASS, -0.0)],
        ] {
            let mut v = with(&amoris_script::access::defaults(&schema), &changes);
            rigid_body_rule(None, &mut v).unwrap_or_else(|r| panic!("{}", r.message));
            assert_eq!(rigid_body_read(&(fns.make)(&v)), v, "{changes:?}");
        }
    }

    fn variants(schema: &ComponentSchema, field: &str) -> Vec<String> {
        let f = schema.fields.iter().find(|f| &*f.name == field).unwrap();
        match &f.ty {
            FieldType::Enum(v) => v.iter().map(|n| n.to_string()).collect(),
            other => panic!("{field} is {other:?}"),
        }
    }

    #[test]
    fn the_kinds_are_the_json_forms_names_in_slot_order() {
        let names = variants(&collider_schema().unwrap(), "kind");
        assert_eq!(names, SHAPE_KINDS);
        for (i, shape) in shapes().iter().enumerate() {
            let tag = match serde_json::to_value(shape).unwrap() {
                Value::Object(m) => m.keys().next().cloned().unwrap(),
                other => panic!("{other}"),
            };
            assert_eq!(tag, names[i]);
            assert_eq!(index(shape_slots(shape)[0]), i);
        }
        let names = variants(&rigid_body_schema().unwrap(), "kind");
        for kind in [BodyKind::Dynamic, BodyKind::Fixed] {
            let slot = usize::try_from(body_kind_slot(kind)).unwrap();
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(names[slot]));
        }
    }

    /// The write trace takes a script field for the whole of the JSON field of its name
    /// (writes.rs, `Slots::resolve`; debugger.md 13), so the fields the two forms share must be
    /// exactly those that hold the same thing: a tag named `shape` would hide a radius change
    /// from `debug writes X.Collider.shape`, a mass named `mass` an inertia change.
    #[test]
    fn a_script_field_named_like_a_json_field_holds_all_of_it() {
        fn shared<C: serde::Serialize>(c: &C, schema: &ComponentSchema) -> Vec<String> {
            let json = serde_json::to_value(c).unwrap();
            let keys = json.as_object().unwrap();
            let fields = schema.fields.iter().map(|f| f.name.to_string());
            fields.filter(|f| keys.contains_key(f)).collect()
        }
        assert_eq!(
            shared(&bodies()[2], &rigid_body_schema().unwrap()),
            ["kind", "linear_damping", "angular_damping", "gravity_scale"]
        );
        assert_eq!(
            shared(&colliders()[1], &collider_schema().unwrap()),
            ["density", "friction", "restitution"]
        );
        assert_eq!(
            shared(&ExternalForce::default(), &external_force_schema().unwrap()),
            ["force", "torque"]
        );
    }
}
