#[path = "common/water_radiation.rs"]
mod experiment;
use amoris_assets::water_field::{FieldBoundary, FieldCheckpoint, FieldRecipe, FieldStep};
use amoris_render::{BackendChoice, Gpu, water_compute::GpuWater, water_resident::ResidentWater};
use std::sync::Arc;

fn advance(
    gpu: &Gpu,
    field: &mut GpuWater,
    edges: &[FieldBoundary; 4],
    dt: f32,
) -> (
    FieldCheckpoint,
    amoris_render::water_compute::BoundaryReport,
) {
    let mut e = gpu.device.create_command_encoder(&Default::default());
    field
        .encode_boundaries(&mut e, 1.0, dt, 1, 0.0, edges)
        .unwrap();
    gpu.queue.submit([e.finish()]);
    let flow = field.read_boundary_report().unwrap();
    (field.checkpoint_complete(0.0).unwrap(), flow)
}
#[test]
fn radiation_preserves_stepped_lakes_and_fills_then_drains_all_four_sides() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("radiation requires GPU");
    gpu.device
        .on_uncaptured_error(Arc::new(|e| panic!("radiation validation: {e}")));
    let cp = FieldCheckpoint::new(
        [16, 16],
        0.0,
        (0..256)
            .map(|n| {
                let b = ((n * 13) % 7) as f32 * 0.1;
                [b, 1.0 - b, 0.0, 0.0].map(f32::to_bits)
            })
            .collect(),
    );
    let mut field = GpuWater::new(&gpu.device, &gpu.queue, cp.size).unwrap();
    field.upload_checkpoint(&cp).unwrap();
    let radiation = FieldBoundary::Radiation {
        surface_relative_m: 1.0,
        outward_velocity_mps: 0.0,
    };
    let (same, flow) = advance(&gpu, &mut field, &[radiation; 4], 0.01);
    assert_eq!(same, cp, "still lake words changed");
    assert_eq!(flow, Default::default());
    for sides in [vec![0], vec![1], vec![2], vec![3], vec![0, 2], vec![1, 3]] {
        let mut cp = FieldCheckpoint::new([16, 16], 0.0, vec![[0; 4]; 256]);
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, cp.size).unwrap();
        field.upload_checkpoint(&cp).unwrap();
        let mut edges = [FieldBoundary::Wall; 4];
        for &s in &sides {
            edges[s] = radiation;
        }
        let mut entered = 0.0;
        let mut left = 0.0;
        for tick in 0..240 {
            if tick == 120 {
                for &s in &sides {
                    edges[s] = FieldBoundary::Radiation {
                        surface_relative_m: 0.0,
                        outward_velocity_mps: 0.0,
                    };
                }
            }
            let (next, r) = advance(&gpu, &mut field, &edges, 0.01);
            cp = next;
            entered += f64::from(r.entered);
            left += f64::from(r.left);
            assert!(
                cp.columns.iter().all(|c| f32::from_bits(c[1]) >= 0.0
                    && c.iter().all(|w| f32::from_bits(*w).is_finite()))
            );
            assert!((experiment::volume(&cp, 1.0) - entered + left).abs() < 0.002);
        }
        assert!(
            entered > 1.0 && left > 0.1,
            "no bidirectional flow {sides:?}: {entered}/{left}"
        );
        eprintln!(
            "radiation wet/dry sides={sides:?}: entered={entered}, left={left}, mass_defect={}",
            experiment::volume(&cp, 1.0) - entered + left
        );
    }
}
#[test]
fn sonic_dry_interface_flux_matches_the_analytic_rarefaction_on_each_side() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("sonic reference requires GPU");
    for side in 0..4 {
        for filling in [false, true] {
            let h = if filling { 0.0f32 } else { 1.0 };
            let cp = FieldCheckpoint::new([16, 16], 0.0, vec![[0, h.to_bits(), 0, 0]; 256]);
            let mut field = GpuWater::new(&gpu.device, &gpu.queue, cp.size).unwrap();
            field.upload_checkpoint(&cp).unwrap();
            let mut edges = [FieldBoundary::Wall; 4];
            edges[side] = FieldBoundary::Radiation {
                surface_relative_m: if filling { 1.0 } else { 0.0 },
                outward_velocity_mps: 0.0,
            };
            let dt = 0.001f32;
            let (next, r) = advance(&gpu, &mut field, &edges, dt);
            let q = (4.0 / 9.0) * (2.0 / 3.0) * 9.81f64.sqrt();
            let expected = q * 16.0 * f64::from(dt);
            let actual = f64::from(if filling { r.entered } else { r.left });
            assert!(
                (actual - expected).abs() < 2e-8,
                "sonic {side}/{filling}: {actual}/{expected}"
            );
            assert!(
                (experiment::volume(&next, 1.0)
                    - experiment::volume(&cp, 1.0)
                    - f64::from(r.entered)
                    + f64::from(r.left))
                .abs()
                    < 1e-6
            );
        }
    }
}
#[test]
fn radiation_steps_reconstruct_exact_resident_fields_after_control_changes() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("radiation replay requires GPU");
    let mut cp = Arc::new(
        FieldCheckpoint::new([16, 16], 0.0, vec![[0, 1.0f32.to_bits(), 0, 0]; 256])
            .checked()
            .unwrap(),
    );
    let mut recipe = FieldRecipe::new(cp.key(), vec![], cp.digest(), String::new());
    let mut resident = ResidentWater::new(&gpu).unwrap();
    let mut entered = 0.0;
    let mut left = 0.0;
    let mut direct = GpuWater::new(&gpu.device, &gpu.queue, cp.size).unwrap();
    direct.upload_checkpoint(&cp).unwrap();
    for tick in 0..70 {
        let edge = FieldBoundary::Radiation {
            surface_relative_m: if tick < 12 { 1.2 } else { 0.8 },
            outward_velocity_mps: if tick % 3 == 0 { 0.1 } else { -0.1 },
        };
        let step = FieldStep {
            exterior: vec![],
            sea: None,
            boundaries: [edge; 4],
            cell_m: 1.0,
            substep_dt: 0.01,
            substeps: 2,
            friction: 0.02,
            calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
            dry_below: 0.0,
            edits: vec![],
            impulses: vec![],
            displacements: vec![],
            physics: Default::default(),
        };
        let out = resident
            .advance(&cp, &recipe, &step, &[[0.5, 0.5], [15.5, 15.5]], false)
            .unwrap();
        entered += f64::from(out.boundary.entered);
        left += f64::from(out.boundary.left);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        direct
            .encode_boundaries(&mut encoder, 1.0, 0.01, 2, 0.02, &[edge; 4])
            .unwrap();
        gpu.queue.submit([encoder.finish()]);
        let checkpoint = direct.checkpoint_complete(0.0).unwrap();
        assert_eq!(checkpoint.digest(), out.summary.digest);
        assert!((experiment::volume(&checkpoint, 1.0) - 256.0 - entered + left).abs() < 0.002);
        if let Some(next) = &out.checkpoint {
            assert_eq!(next.as_raw(), &checkpoint);
            cp = next.clone();
        }
        let json = serde_json::to_vec(&out.recipe).unwrap();
        recipe = serde_json::from_slice(&json).unwrap();
        let mut fresh = ResidentWater::new(&gpu).unwrap();
        let restored = fresh
            .observe(&cp, &recipe, &[[0.5, 0.5], [15.5, 15.5]])
            .unwrap();
        assert_eq!(restored.summary.digest, out.summary.digest);
        assert_eq!(restored.samples, out.samples);
    }
    assert!(entered > 0.0 && left > 0.0);
}
#[test]
fn outgoing_packet_reflects_less_than_a_wall_or_an_outward_only_stage() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("wave reflection requires GPU");
    for side in 0..4 {
        let background = experiment::pulse_domain(&gpu, side, 128, 64.0, FieldBoundary::Wall);
        let wall = experiment::pulse(&gpu, side, 64, FieldBoundary::Wall);
        let stage = experiment::pulse(
            &gpu,
            side,
            64,
            FieldBoundary::Stage {
                surface_relative_m: 1.0,
            },
        );
        let radiation = experiment::pulse(
            &gpu,
            side,
            64,
            FieldBoundary::Radiation {
                surface_relative_m: 1.0,
                outward_velocity_mps: 0.0,
            },
        );
        let wall_return = experiment::return_difference(&wall, &background);
        let stage_return = experiment::return_difference(&stage, &background);
        let radiation_return = experiment::return_difference(&radiation, &background);
        eprintln!(
            "reflection side={side}: wall_normalized_stage={}, wall_normalized_radiation={}",
            (stage_return / wall_return).sqrt(),
            (radiation_return / wall_return).sqrt()
        );
        assert!(
            wall.incident_energy > 1e-6 && wall_return > 1e-6,
            "wave was not exercised"
        );
        assert!(radiation_return < wall_return * 0.04);
        assert!(radiation_return < stage_return * 0.05);
        for r in [&wall, &stage, &radiation] {
            assert!(r.max_mass_error_m3 < 0.001);
        }
    }
}

#[test]
fn excessive_reservoir_speed_returns_failure_instead_of_clipping_a_successful_step() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("forcing guard requires GPU");
    let cp = Arc::new(
        FieldCheckpoint::new([16, 16], 0.0, vec![[0, 1.0f32.to_bits(), 0, 0]; 256])
            .checked()
            .unwrap(),
    );
    let recipe = FieldRecipe::new(cp.key(), vec![], cp.digest(), String::new());
    let step = FieldStep {
        exterior: vec![],
        sea: None,
        boundaries: [FieldBoundary::Radiation {
            surface_relative_m: 1.0,
            outward_velocity_mps: -1000.0,
        }; 4],
        cell_m: 1.0,
        substep_dt: 0.01,
        substeps: 1,
        friction: 0.0,
        calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
        dry_below: 0.0,
        edits: vec![],
        impulses: vec![],
        displacements: vec![],
        physics: Default::default(),
    };
    let mut resident = ResidentWater::new(&gpu).unwrap();
    let Err(error) = resident.advance(&cp, &recipe, &step, &[], false) else {
        panic!("too-fast characteristic forcing silently succeeded")
    };
    assert!(error.contains("boundary"), "wrong failure: {error}");
    eprintln!("explicit reservoir rejection: {error}");
}
