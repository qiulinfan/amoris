//! Hull field ownership and resident scheduling with a static identity host. The mock omits
//! fluid transport; native/browser GPU checks establish numerical execution separately.
use amoris_physics::water::{
    self,
    compute::{ComputeQueue, Job, ResidentBatchJob, ResidentOutput, ResidentResult, ResidentStats},
    feedback::WaterFeedback,
    resident::WaterProbe,
};
use amoris_physics::{
    Boat, BuoyPoint, Collider, ExternalForce, Floater, Hull, MassProps, RigidBody, Sea, Shape,
    Transform, Velocity,
};
use amoris_sim::{EntityId, NoHooks, Sim, SimConfig, StepHooks, SystemKey, TickPhase, entity};

fn world() -> Sim {
    let mut s = Sim::new(SimConfig {
        rate: amoris_sim::TickRate::DEFAULT,
        seed: 7,
    })
    .unwrap();
    amoris_physics::plugin(&mut s).unwrap();
    s
}
fn field(
    s: &mut Sim,
    solver: water::Solver,
    origin: [f64; 3],
    surface: f64,
    depth: f64,
    feedback: bool,
) -> EntityId {
    let mut r = water::dam_scene(1);
    r.solver = solver;
    r.cell_m = 1.0;
    for tile in &mut r.tiles {
        tile.bed.fill(surface - depth);
        tile.depth.fill(depth);
        tile.u.fill(0.0);
        tile.w.fill(0.0);
    }
    if feedback {
        s.boundary()
            .spawn((r, Transform::at(origin), WaterFeedback::default()))
            .unwrap()
    } else {
        s.boundary().spawn((r, Transform::at(origin))).unwrap()
    }
}
fn body(s: &mut Sim, at: [f64; 3], hull: Hull, velocity: Velocity) -> EntityId {
    s.boundary()
        .spawn((
            Transform::at(at),
            velocity,
            RigidBody::dynamic().with_mass(MassProps::solid_box(100.0, [0.2; 3], [0.0; 3])),
            Collider::new(Shape::Cuboid {
                half_extents: [0.2; 3],
            }),
            Boat::default(),
            hull,
            ExternalForce::default(),
        ))
        .unwrap()
}
fn hull() -> Hull {
    Hull {
        keel_at: [0.0, -0.55, 0.0],
        keel_coef: 0.0,
        rudder_at: [0.0, -0.3, 1.0],
        rudder_coef: 0.0,
        rudder_max_deg: 34.0,
        drag_coef: 5.0,
    }
}
struct Capture {
    id: EntityId,
    force: Option<ExternalForce>,
}
impl Capture {
    fn new(id: EntityId) -> Self {
        Self { id, force: None }
    }
}
impl StepHooks for Capture {
    fn world(
        &mut self,
        key: &SystemKey,
        _: TickPhase,
        begin: bool,
        world: &bevy_ecs::world::World,
    ) {
        if !begin && key.as_str() == "physics.hull" {
            self.force = world
                .get::<ExternalForce>(entity::require(world, self.id).unwrap())
                .copied();
        }
    }
}
fn step_force(s: &mut Sim, id: EntityId) -> ExternalForce {
    let mut hook = Capture::new(id);
    let report = s.step(&mut hook).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    hook.force.unwrap()
}
fn take(q: &ComputeQueue) -> ResidentBatchJob {
    let Job::Resident(job) = q.take_job().unwrap().expect("resident batch") else {
        panic!("grid job");
    };
    *job
}
fn complete(q: &ComputeQueue, job: &ResidentBatchJob) -> bool {
    let out = job
        .requests
        .iter()
        .map(|r| {
            let mut recipe = r.recipe.clone();
            if let Some(step) = &r.step {
                recipe.push_step(step.clone());
            }
            recipe.environment = "static hull test host".into();
            let f = &r.checkpoint;
            let samples = r
                .points
                .iter()
                .map(|p| {
                    let n = p[0].floor() as usize + p[1].floor() as usize * f.size[0] as usize;
                    let c = f.columns[n].map(f32::from_bits).map(f64::from);
                    [f.datum + c[0] + c[1], c[1], c[2], c[3]]
                })
                .collect();
            let depth = f.columns.iter().map(|c| f64::from(f32::from_bits(c[1])));
            let stats = ResidentStats {
                max_depth: depth.clone().fold(0.0, amoris_sim::math::max),
                volume_cells: depth.sum(),
                ..Default::default()
            };
            let checkpoint = if recipe.steps().len() == amoris_assets::water_field::MAX_STEPS {
                recipe.reset(f.key(), f.digest());
                Some(f.clone())
            } else {
                None
            };
            ResidentResult {
                region: r.region,
                output: ResidentOutput {
                    recipe,
                    stats,
                    samples,
                    checkpoint,
                    reaction: Default::default(),
                    boundary: Default::default(),
                },
            }
        })
        .collect();
    q.complete_resident(job.id, Ok(out)).unwrap()
}

#[test]
fn field_drag_acts_with_a_wet_keel_even_when_floating_center_is_above_surface() {
    let mut s = world();
    field(&mut s, water::Solver::Cpu, [0.0; 3], 2.0, 2.0, false);
    let id = body(
        &mut s,
        [8.0, 2.2, 8.0],
        hull(),
        Velocity {
            linear: [0.0, 0.0, -2.0],
            ..Default::default()
        },
    );
    let f = step_force(&mut s, id);
    assert_eq!(f.force, [0.0, 0.0, 20.0]);
}
#[test]
fn dry_covered_center_and_below_bed_points_mask_both_other_fields_and_sea() {
    for (depth, surface, y) in [(0.0, 0.0, 1.0), (1.0, 2.0, 0.4)] {
        let mut s = world();
        field(&mut s, water::Solver::Cpu, [0.0; 3], surface, depth, false);
        s.boundary()
            .spawn((Sea {
                level: 10.0,
                ..Default::default()
            },))
            .unwrap();
        let id = body(
            &mut s,
            [8.0, y, 8.0],
            Hull {
                keel_coef: 10.0,
                rudder_coef: 10.0,
                ..hull()
            },
            Velocity {
                linear: [-2.0, 0.0, -2.0],
                ..Default::default()
            },
        );
        assert_eq!(step_force(&mut s, id).force, [0.0; 3]);
    }
    let mut s = world();
    field(&mut s, water::Solver::Cpu, [8.0, 0.0, 0.0], 0.0, 0.0, false);
    field(&mut s, water::Solver::Cpu, [0.0; 3], 2.0, 2.0, false);
    let id = body(
        &mut s,
        [8.5, 1.0, 8.5],
        Hull {
            keel_at: [-2.0, -0.55, 0.0],
            ..hull()
        },
        Velocity {
            linear: [0.0, 0.0, -2.0],
            ..Default::default()
        },
    );
    assert_eq!(
        step_force(&mut s, id).force,
        [0.0; 3],
        "wet keel cannot borrow resistance across a dry center column"
    );
}
#[test]
fn foil_layer_is_clipped_to_thin_water_and_uncovered_points_keep_sea() {
    let mut s = world();
    field(&mut s, water::Solver::Cpu, [0.0; 3], 1.0, 0.05, false);
    let id = body(
        &mut s,
        [8.0, 0.95, 8.0],
        Hull {
            keel_at: [0.0; 3],
            keel_coef: 10.0,
            drag_coef: 0.0,
            ..hull()
        },
        Velocity {
            linear: [-1.0, 0.0, 0.0],
            ..Default::default()
        },
    );
    let f = step_force(&mut s, id);
    assert!((f.force[0] - 5.0).abs() < 1e-12, "{f:?}");
    let mut s = world();
    field(&mut s, water::Solver::Cpu, [0.0; 3], 0.0, 0.0, false);
    s.boundary()
        .spawn((Sea {
            level: 2.0,
            ..Default::default()
        },))
        .unwrap();
    let id = body(
        &mut s,
        [20.0, 2.2, 8.0],
        hull(),
        Velocity {
            linear: [0.0, 0.0, -2.0],
            ..Default::default()
        },
    );
    assert_eq!(step_force(&mut s, id).force, [0.0, 0.0, 20.0]);
}
#[test]
fn rudder_point_velocity_includes_angular_motion() {
    let mut s = world();
    field(&mut s, water::Solver::Cpu, [0.0; 3], 2.0, 2.0, false);
    let id = body(
        &mut s,
        [8.0, 1.0, 8.0],
        Hull {
            rudder_at: [0.0, 0.0, 1.0],
            rudder_coef: 20.0,
            drag_coef: 0.0,
            ..hull()
        },
        Velocity {
            angular: [0.0, 1.0, 0.0],
            ..Default::default()
        },
    );
    let f = step_force(&mut s, id);
    assert_eq!(f.force, [-20.0, 0.0, 0.0]);
    assert_eq!(f.torque[1], -20.0);
}
#[test]
fn resident_hull_has_three_exact_points_distinct_from_floater_keys_and_feedback_queues_only_new_hull_force()
 {
    let mut s = world();
    let q = ComputeQueue::default();
    water::compute::install(&mut s, q.backend());
    let region = field(&mut s, water::Solver::GpuResident, [0.0; 3], 2.0, 2.0, true);
    let id = body(
        &mut s,
        [8.0, 1.0, 8.0],
        Hull {
            keel_at: [-1.0, -0.2, 0.0],
            keel_coef: 10.0,
            rudder_at: [1.0, -0.1, 1.0],
            rudder_coef: 20.0,
            ..hull()
        },
        Velocity {
            linear: [-1.0, 0.0, -2.0],
            ..Default::default()
        },
    );
    let e = entity::require(s.world(), id).unwrap();
    s.world_mut().entity_mut(e).insert(Floater {
        points: vec![BuoyPoint {
            at: [2.0, 0.0, 0.0],
            volume: 0.001,
            size: 0.2,
        }],
        drag: 0.0,
        heave: 0.0,
        submerged: 0.0,
    });
    s.world_mut().get_mut::<ExternalForce>(e).unwrap().force = [7.0, 0.0, 11.0];
    let mut hook = Capture::new(id);
    assert!(s.poll_step(&mut hook).unwrap().is_pending());
    let job = take(&q);
    assert_eq!(
        job.requests[0].points,
        [[10.0, 8.0], [7.0, 8.0], [9.0, 9.0], [8.0, 8.0]]
    );
    assert_eq!(job.requests[0].coupled_from, Some(0));
    assert!(complete(&q, &job));
    assert!(s.poll_step(&mut hook).unwrap().is_ready());
    assert_eq!(hook.force.unwrap().force[2], 31.0);
    let feedback = s
        .world()
        .get::<WaterFeedback>(entity::require(s.world(), region).unwrap())
        .unwrap();
    assert_eq!(
        feedback.pending.len(),
        3,
        "one new Hull force reaction at each actual point"
    );
    for (packet, at, impulse) in [
        (&feedback.pending[0], [7.0, 8.0], [-10.0 / 60.0, 0.0]),
        (&feedback.pending[1], [9.0, 9.0], [-20.0 / 60.0, 0.0]),
        (&feedback.pending[2], [8.0, 8.0], [0.0, -20.0 / 60.0]),
    ] {
        assert_eq!(packet.at, at);
        assert_eq!(
            packet.impulse, impulse,
            "no prior force or Floater redistribution enters Hull reaction"
        );
    }
    let pending = feedback.pending.clone();
    assert!(s.poll_step(&mut NoHooks).unwrap().is_pending());
    let next = take(&q);
    assert_eq!(next.requests[0].step.as_ref().unwrap().impulses, pending);
}
#[test]
fn async_pose_changes_are_rejected_and_restore_hook_cancels_late_hull_samples() {
    for cancel in [false, true] {
        let mut s = world();
        let q = ComputeQueue::default();
        water::compute::install(&mut s, q.backend());
        field(
            &mut s,
            water::Solver::GpuResident,
            [0.0; 3],
            2.0,
            2.0,
            false,
        );
        let id = body(&mut s, [8.0, 1.0, 8.0], hull(), Velocity::default());
        assert!(s.poll_step(&mut NoHooks).unwrap().is_pending());
        let job = take(&q);
        let e = entity::require(s.world(), id).unwrap();
        if cancel {
            water::compute::invalidate(s.world_mut());
            assert!(!complete(&q, &job));
        } else {
            s.world_mut().get_mut::<Transform>(e).unwrap().position[1] += 0.1;
            assert!(complete(&q, &job));
            assert_eq!(
                s.poll_step(&mut NoHooks).unwrap_err().code,
                "water.hull_failed"
            );
        }
    }
}
#[test]
fn probes_and_all_three_hull_points_share_the_per_region_capacity() {
    for probes in [4093, 4094] {
        let mut s = world();
        let q = ComputeQueue::default();
        water::compute::install(&mut s, q.backend());
        let region = field(
            &mut s,
            water::Solver::GpuResident,
            [0.0; 3],
            2.0,
            2.0,
            false,
        );
        for _ in 0..probes {
            s.boundary()
                .spawn((WaterProbe {
                    region: Some(region),
                    at: [1.0, 1.0],
                    ..Default::default()
                },))
                .unwrap();
        }
        body(&mut s, [8.0, 1.0, 8.0], hull(), Velocity::default());
        if probes == 4093 {
            assert!(s.poll_step(&mut NoHooks).unwrap().is_pending());
            assert_eq!(take(&q).requests[0].points.len(), 4096);
        } else {
            let error = s.poll_step(&mut NoHooks).unwrap_err();
            assert_eq!(error.code, "water.resident_failed");
            assert!(q.take_job().unwrap().is_none());
        }
    }
}

#[test]
fn field_current_drives_a_stationary_hull_without_a_sea_or_paddle() {
    let mut s = world();
    let region = field(&mut s, water::Solver::Cpu, [0.0; 3], 2.0, 2.0, false);
    let e = entity::require(s.world(), region).unwrap();
    for tile in &mut s
        .world_mut()
        .get_mut::<water::WaterRegion>(e)
        .unwrap()
        .tiles
    {
        tile.u.fill(1.0);
    }
    let id = body(
        &mut s,
        [8.0, 1.0, 8.0],
        Hull {
            keel_coef: 10.0,
            drag_coef: 0.0,
            ..hull()
        },
        Velocity::default(),
    );
    let f = step_force(&mut s, id);
    assert!(f.force[0] > 9.0 && f.force[0] < 10.1, "{f:?}");
    let e = entity::require(s.world(), id).unwrap();
    assert!(s.world().get::<Velocity>(e).unwrap().linear[0] > 0.0);
}

#[test]
fn a_missing_hull_sample_stops_before_any_force_or_body_integration() {
    let mut s = world();
    let q = ComputeQueue::default();
    water::compute::install(&mut s, q.backend());
    field(
        &mut s,
        water::Solver::GpuResident,
        [0.0; 3],
        2.0,
        2.0,
        false,
    );
    let id = body(&mut s, [8.0, 1.0, 8.0], hull(), Velocity::default());
    let e = entity::require(s.world(), id).unwrap();
    let before = *s.world().get::<Transform>(e).unwrap();
    let mut hook = Capture::new(id);
    assert!(s.poll_step(&mut hook).unwrap().is_pending());
    let job = take(&q);
    let request = &job.requests[0];
    let mut recipe = request.recipe.clone();
    recipe.push_step(request.step.clone().unwrap());
    recipe.environment = "static hull test host".into();
    assert!(
        q.complete_resident(
            job.id,
            Ok(vec![ResidentResult {
                region: request.region,
                output: ResidentOutput {
                    recipe,
                    stats: ResidentStats {
                        max_depth: 2.0,
                        volume_cells: 512.0,
                        ..Default::default()
                    },
                    samples: vec![[2.0, 2.0, 0.0, 0.0]; 2],
                    checkpoint: None,
                    reaction: Default::default(),
                    boundary: Default::default()
                }
            }])
        )
        .unwrap()
    );
    assert_eq!(
        s.poll_step(&mut hook).unwrap_err().code,
        "water.resident_failed"
    );
    assert!(hook.force.is_none());
    assert_eq!(*s.world().get::<Transform>(e).unwrap(), before);
}
