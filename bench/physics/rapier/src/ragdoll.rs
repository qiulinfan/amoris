//! Jolt's RagdollScene rebuilt in Rapier from what Jolt itself created: `jolt_bench --scene Ragdoll
//! --export ragdoll.txt` writes the 160 ragdolls (23 parts each, 3,680 bodies and 3,520 swing-twist
//! constraints driven to a dead pose by motors) and the Horizon Zero Dawn terrain piece Jolt loads
//! (457 static bodies holding 5,786 leaf shapes, 2.7 million triangles). The format is jolt_bench.cpp's ExportScene.
//!
//! Each body is placed at Jolt's centre of mass with Jolt's rotation and gets Jolt's mass and
//! principal inertia; its colliders carry no mass. Joint frames are Jolt's constraint frames, which
//! are relative to the centre of mass too. Rapier has no swing-twist joint: the spherical joint gets
//! per-axis limits (twist on X, Jolt's plane half-cone on Y, its normal half-cone on Z, a box where
//! Jolt has an elliptic cone) and three position motors whose targets are Jolt's target orientation
//! measured the way Rapier measures an angle (2 asin of the quaternion's component). Jolt's tapered
//! capsules become capsules of their mean radius (Rapier has no tapered capsule).

use std::fs;
use std::path::Path;

use rapier3d::prelude::*;

struct Tokens<'a> {
    it: std::str::SplitAsciiWhitespace<'a>,
}

impl<'a> Tokens<'a> {
    fn word(&mut self) -> &'a str {
        self.it.next().expect("unexpected end of the scene file")
    }
    fn f(&mut self) -> f32 {
        self.word().parse().expect("a number")
    }
    fn u(&mut self) -> usize {
        self.word().parse().expect("a count")
    }
    fn expect(&mut self, w: &str) {
        let got = self.word();
        assert_eq!(got, w, "scene file: expected {w}");
    }
    fn vec(&mut self) -> Vector {
        Vector::new(self.f(), self.f(), self.f())
    }
    fn quat(&mut self) -> Rotation {
        let (x, y, z, w) = (self.f(), self.f(), self.f(), self.f());
        Rotation::from_xyzw(x, y, z, w).normalize()
    }
}

/// Which sub-group pairs inside one ragdoll must not touch, beyond the jointed parent-child pairs
/// (those are the joints' `contacts_enabled(false)`). Collider user data: ragdoll << 8 | part.
pub struct RagdollFilter {
    parts: usize,
    off: Vec<bool>,
}

impl PhysicsHooks for RagdollFilter {
    fn filter_contact_pair(&self, ctx: &PairFilterContext) -> Option<SolverFlags> {
        let a = ctx.colliders[ctx.collider1].user_data;
        let b = ctx.colliders[ctx.collider2].user_data;
        // Terrain colliders have user data 0; ragdoll parts have bit 32 set.
        if a >> 32 == 1 && b >> 32 == 1 && (a >> 8) == (b >> 8) {
            let (i, j) = ((a & 0xff) as usize, (b & 0xff) as usize);
            if self.off[i * self.parts + j] {
                return None;
            }
        }
        Some(SolverFlags::COMPUTE_RIGID_IMPULSES)
    }
}

pub struct RagdollStats {
    pub statics: usize,
    pub triangles: usize,
    pub bodies: usize,
    pub joints: usize,
}

/// How the ragdoll joints are built. `multibody`: Rapier multibody joints (reduced coordinates, each
/// ragdoll one articulation rooted at its first part) instead of impulse joints, with the same
/// frames, limits and motors. `motors` and `limits` off drop the three position motors or the three
/// angular limits (diagnosis only; the benchmark keeps both).
#[derive(Clone, Copy)]
pub struct JointOptions {
    pub multibody: bool,
    pub motors: bool,
    pub limits: bool,
}

pub fn build(
    world: &mut PhysicsWorld,
    path: &Path,
    no_sleep: bool,
    opts: JointOptions,
) -> (RagdollFilter, RagdollStats) {
    let text = fs::read_to_string(path).expect("ragdoll scene file (jolt_bench --export)");
    let tri_bytes = fs::read(path.with_extension("txt.tri")).expect("ragdoll triangles (.tri)");
    let mut t = Tokens {
        it: text.split_ascii_whitespace(),
    };

    // Static leaf shapes in world space: a trimesh each, or a convex hull for convex leaves. Jolt
    // groups them into 457 static bodies (compounds of scaled meshes); here each is its own fixed
    // collider, so Rapier's broad phase holds 5,786 static entries where Jolt's holds 457.
    t.expect("statics");
    let nstatics = t.u();
    let ntris = t.u();
    assert_eq!(tri_bytes.len(), ntris * 36);
    let floats: Vec<f32> = tri_bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let mut first = 0usize;
    let mut statics = 0;
    for _ in 0..nstatics {
        let convex = t.u() == 1;
        let count = t.u();
        if count == 0 {
            continue;
        }
        let verts: Vec<Vector> = (first * 3..(first + count) * 3)
            .map(|v| Vector::new(floats[v * 3], floats[v * 3 + 1], floats[v * 3 + 2]))
            .collect();
        first += count;
        let co = if convex {
            ColliderBuilder::convex_hull(&verts).expect("convex leaf")
        } else {
            let indices: Vec<[u32; 3]> = (0..count as u32)
                .map(|i| [3 * i, 3 * i + 1, 3 * i + 2])
                .collect();
            // Jolt's meshes have active-edge detection on; FIX_INTERNAL_EDGES is Rapier's
            // equivalent (it also welds the duplicated vertices of this triangle soup).
            ColliderBuilder::trimesh_with_flags(verts, indices, TriMeshFlags::FIX_INTERNAL_EDGES)
                .expect("terrain mesh")
        };
        world.insert(RigidBodyBuilder::fixed(), co.friction(0.2));
        statics += 1;
    }

    // Dynamic bodies.
    t.expect("bodies");
    let nbodies = t.u();
    let mut handles = Vec::with_capacity(nbodies);
    let mut groups = Vec::with_capacity(nbodies);
    for _ in 0..nbodies {
        t.expect("body");
        let com = t.vec();
        let rot = t.quat();
        let mass = t.f();
        let inv_inertia = t.vec();
        let inertia_rot = t.quat();
        let friction = t.f();
        let restitution = t.f();
        let lin_damp = t.f();
        let ang_damp = t.f();
        let _max_ang_vel = t.f();
        let group = t.u();
        let sub_group = t.u();
        let can_sleep = t.u() == 1;
        let leaves = t.u();
        let inertia = Vector::new(
            1.0 / inv_inertia.x,
            1.0 / inv_inertia.y,
            1.0 / inv_inertia.z,
        );
        let rb = RigidBodyBuilder::dynamic()
            .pose(Pose::from_parts(com, rot))
            .linear_damping(lin_damp)
            .angular_damping(ang_damp)
            .can_sleep(can_sleep && !no_sleep)
            .additional_mass_properties(MassProperties::with_principal_inertia_frame(
                Vector::ZERO,
                mass,
                inertia,
                inertia_rot,
            ));
        let h = world.insert_body(rb);
        for _ in 0..leaves {
            let kind = t.word();
            let co = match kind {
                "box" => {
                    let he = t.vec();
                    let _convex_radius = t.f();
                    let p = t.vec();
                    let q = t.quat();
                    ColliderBuilder::cuboid(he.x, he.y, he.z).position(Pose::from_parts(p, q))
                }
                "sphere" => {
                    let r = t.f();
                    let p = t.vec();
                    ColliderBuilder::ball(r).translation(p)
                }
                "capsule" => {
                    let a = t.vec();
                    let b = t.vec();
                    let (ra, rb) = (t.f(), t.f());
                    ColliderBuilder::capsule_from_endpoints(a, b, 0.5 * (ra + rb))
                }
                other => panic!("unsupported leaf shape {other}"),
            };
            let co = co
                .density(0.0)
                .friction(friction)
                .restitution(restitution)
                .user_data((1u128 << 32) | ((group as u128) << 8) | sub_group as u128)
                .active_hooks(ActiveHooks::FILTER_CONTACT_PAIRS);
            world.insert_collider(co, Some(h));
        }
        handles.push(h);
        groups.push(sub_group);
    }

    t.expect("nocollide");
    let npairs = t.u();
    let parts = groups.iter().copied().max().unwrap_or(0) + 1;
    let mut off = vec![false; parts * parts];
    for _ in 0..npairs {
        let (i, j) = (t.u(), t.u());
        off[i * parts + j] = true;
        off[j * parts + i] = true;
    }

    t.expect("swingtwist");
    let njoints = t.u();
    for _ in 0..njoints {
        let (b1, b2) = (t.u(), t.u());
        let p1 = t.vec();
        let q1 = t.quat();
        let p2 = t.vec();
        let q2 = t.quat();
        let normal_half_cone = t.f();
        let plane_half_cone = t.f();
        let twist_min = t.f();
        let twist_max = t.f();
        let swing = (t.u(), t.f(), t.f(), t.f());
        let twist = (t.u(), t.f(), t.f(), t.f());
        let (swing_state, twist_state) = (t.u(), t.u());
        let mut target = t.quat();
        let _max_friction_torque = t.f();
        if target.w < 0.0 {
            target = -target;
        }
        // Jolt's constraint space: X twist, Y normal, Z plane axis; its swing about Y is limited by
        // the plane half-cone and about Z by the normal half-cone (SwingTwistConstraint.cpp).
        let mut j = GenericJointBuilder::new(JointAxesMask::LOCKED_SPHERICAL_AXES)
            .local_frame1(Pose::from_parts(p1, q1))
            .local_frame2(Pose::from_parts(p2, q2))
            .contacts_enabled(false);
        if opts.limits {
            j = j
                .limits(JointAxis::AngX, [twist_min, twist_max])
                .limits(JointAxis::AngY, [-plane_half_cone, plane_half_cone])
                .limits(JointAxis::AngZ, [-normal_half_cone, normal_half_cone]);
        }
        // Position motors (Jolt EMotorState::Position = 2) as springs of Jolt's frequency and
        // damping ratio; Rapier's acceleration-based model takes stiffness w^2 and damping 2 z w.
        // libm, not std: std's asin calls the platform's C library natively, and the native and
        // wasm32 runs then diverge from the first step (seen here before this used libm).
        let target_angle = |c: f32| 2.0 * libm::asinf(c.clamp(-1.0, 1.0));
        let motor = |j: GenericJointBuilder,
                     axis,
                     target: f32,
                     (_mode, freq, zeta, max_torque): (usize, f32, f32, f32)| {
            let w = 2.0 * std::f32::consts::PI * freq;
            j.motor_position(axis, target, w * w, 2.0 * zeta * w)
                .motor_max_force(axis, max_torque)
        };
        if opts.motors && twist_state == 2 {
            j = motor(j, JointAxis::AngX, target_angle(target.x), twist);
        }
        if opts.motors && swing_state == 2 {
            j = motor(j, JointAxis::AngY, target_angle(target.y), swing);
            j = motor(j, JointAxis::AngZ, target_angle(target.z), swing);
        }
        if opts.multibody {
            world
                .insert_multibody_joint(handles[b1], handles[b2], j)
                .expect("a ragdoll is a tree of joints");
        } else {
            world.insert_impulse_joint(handles[b1], handles[b2], j);
        }
    }
    t.expect("end");
    (
        RagdollFilter { parts, off },
        RagdollStats {
            statics,
            triangles: ntris,
            bodies: nbodies,
            joints: njoints,
        },
    )
}
