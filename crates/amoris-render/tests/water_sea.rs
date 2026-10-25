use amoris_assets::water_field::{
    FieldBoundary, FieldCheckpoint, FieldRecipe, FieldSea, FieldSeaWave, FieldStep,
};
use amoris_render::{BackendChoice, Gpu, water_compute::GpuWater, water_resident::ResidentWater};
use std::sync::Arc;
fn wave() -> FieldSeaWave {
    let k = std::f64::consts::TAU / 16.0;
    FieldSeaWave {
        amplitude_m: 0.005,
        wave_vector_rad_m: [k * 0.8, k * 0.6],
        angular_frequency_rad_s: (9.81 * k).sqrt(),
        phase_at_start_rad: 0.7,
    }
}
fn step(substeps: u32, dt: f64) -> FieldStep {
    FieldStep {
        exterior: vec![],
        boundaries: [
            FieldBoundary::Sea,
            FieldBoundary::Radiation {
                surface_relative_m: 0.125,
                outward_velocity_mps: 0.0,
            },
            FieldBoundary::Wall,
            FieldBoundary::Wall,
        ],
        sea: Some(FieldSea {
            level_relative_m: 0.125,
            waves: vec![wave()],
        }),
        cell_m: 0.25,
        substep_dt: dt,
        substeps,
        friction: 0.0,
        calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
        dry_below: 0.0,
        edits: vec![],
        impulses: vec![],
        displacements: vec![],
        physics: Default::default(),
    }
}
fn checkpoint() -> FieldCheckpoint {
    FieldCheckpoint::new([16, 16], 0.0, vec![[0, 0.125f32.to_bits(), 0, 0]; 256])
}
fn advance(
    gpu: &Gpu,
    field: &mut GpuWater,
    step: &FieldStep,
) -> amoris_render::water_compute::BoundaryReport {
    let mut e = gpu.device.create_command_encoder(&Default::default());
    field.encode_transport(&mut e, step, None).unwrap();
    gpu.queue.submit([e.finish()]);
    field.read_boundary_report().unwrap()
}
fn volume(cp: &FieldCheckpoint) -> f64 {
    cp.columns
        .iter()
        .map(|c| f64::from(f32::from_bits(c[1])))
        .sum::<f64>()
        * 0.0625
}
#[test]
fn spatial_sea_face_states_match_independent_f64_characteristics_and_mass_flux() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("sea requires hardware GPU");
    gpu.device
        .on_uncaptured_error(Arc::new(|e| panic!("sea validation: {e}")));
    for side in 0..4usize {
        let initial = checkpoint();
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, initial.size).unwrap();
        field.upload_checkpoint(&initial).unwrap();
        let mut s = step(1, 0.001);
        s.boundaries = [FieldBoundary::Wall; 4];
        s.boundaries[side] = FieldBoundary::Sea;
        let axis = usize::from(side >= 2);
        let sign = if side % 2 == 0 { -1.0f64 } else { 1.0 };
        let k = std::f64::consts::TAU / 16.0;
        let w = &mut s.sea.as_mut().unwrap().waves[0];
        w.wave_vector_rad_m[axis] = -sign * k * 0.8;
        w.wave_vector_rad_m[1 - axis] = k * 0.6;
        let w = *w;
        let flow = advance(&gpu, &mut field, &s);
        let actual = field.checkpoint_complete(0.0).unwrap();
        let c = (9.81 * 0.125f64).sqrt();
        let normal = ((w.angular_frequency_rad_s / c).powi(2)
            - w.wave_vector_rad_m[1 - axis].powi(2))
        .sqrt();
        let mut entered = 0.0;
        let mut left = 0.0;
        let mut error = 0.0f64;
        for n in 0..16usize {
            let mut pos = [0.0; 2];
            pos[axis] = if sign < 0.0 { 0.0 } else { 4.0 };
            pos[1 - axis] = (n as f64 + 0.5) * 0.25;
            let eta = w.amplitude_m
                * (w.wave_vector_rad_m[0] * pos[0] + w.wave_vector_rad_m[1] * pos[1]
                    - w.angular_frequency_rad_s * 0.0005
                    + w.phase_at_start_rad)
                    .sin();
            let ue = -eta * 9.81 * normal / w.angular_frequency_rad_s;
            let ce = (9.81 * (0.125 + eta)).sqrt();
            let ub = (2.0 * c + ue - 2.0 * ce) * 0.5;
            let cb = (2.0 * c - ue + 2.0 * ce) * 0.25;
            let q = ub * cb * cb / 9.81;
            entered += (-q).max(0.0) * 0.25 * 0.001;
            left += q.max(0.0) * 0.25 * 0.001;
            let stored = match side {
                0 => actual.columns[n * 16][2],
                1 => actual.far_faces[n],
                2 => actual.columns[n][3],
                _ => actual.far_faces[16 + n],
            };
            error = error.max((f64::from(f32::from_bits(stored)) - sign * ub).abs());
        }
        assert!(error < 3e-7, "per-face velocity {side}: {error}");
        assert!((f64::from(flow.entered) - entered).abs() < 3e-10);
        assert!((f64::from(flow.left) - left).abs() < 3e-10);
        assert!(
            (volume(&actual) - volume(&initial) - f64::from(flow.entered) + f64::from(flow.left))
                .abs()
                < 2e-7
        );
        eprintln!(
            "sea f64 side={side}: velocity_error={error}, entered_error={}, left_error={}",
            (f64::from(flow.entered) - entered).abs(),
            (f64::from(flow.left) - left).abs()
        );
    }
}
#[test]
fn batched_substeps_use_distinct_midpoint_phases_and_reset_their_disposable_clock() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("sea clock requires GPU");
    let initial = checkpoint();
    let mut batch = GpuWater::new(&gpu.device, &gpu.queue, initial.size).unwrap();
    batch.upload_checkpoint(&initial).unwrap();
    let mut serial = GpuWater::new(&gpu.device, &gpu.queue, initial.size).unwrap();
    serial.upload_checkpoint(&initial).unwrap();
    let s = step(8, 0.05);
    let flow = advance(&gpu, &mut batch, &s);
    let mut entered = 0.0;
    let mut left = 0.0;
    for n in 0..8 {
        let mut one = step(1, 0.05);
        one.sea.as_mut().unwrap().waves[0].phase_at_start_rad -=
            wave().angular_frequency_rad_s * n as f64 * 0.05;
        let f = advance(&gpu, &mut serial, &one);
        entered += f64::from(f.entered);
        left += f64::from(f.left);
    }
    let a = batch.checkpoint_complete(0.0).unwrap();
    let b = serial.checkpoint_complete(0.0).unwrap();
    let error = a
        .columns
        .iter()
        .flatten()
        .zip(b.columns.iter().flatten())
        .map(|(a, b)| (f32::from_bits(*a) - f32::from_bits(*b)).abs())
        .fold(0.0f32, f32::max);
    assert!(error < 1e-6, "batched midpoint phases {error}");
    assert!((f64::from(flow.entered) - entered).abs() < 1e-7);
    assert!((f64::from(flow.left) - left).abs() < 1e-7);
    batch.upload_checkpoint(&initial).unwrap();
    advance(&gpu, &mut batch, &s);
    assert_eq!(
        batch.checkpoint_complete(0.0).unwrap(),
        a,
        "clock leaked across submissions"
    );
    eprintln!("sea midpoint state error={error}");
}
#[test]
fn deep_wavelength_alone_does_not_bypass_the_local_shallow_resolution_filter() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("sea resolution requires GPU");
    let initial = FieldCheckpoint::new([16, 16], 0.0, vec![[0, 0.05f32.to_bits(), 0, 0]; 256]);
    let mut field = GpuWater::new(&gpu.device, &gpu.queue, initial.size).unwrap();
    field.upload_checkpoint(&initial).unwrap();
    let mut s = step(4, 0.01);
    s.cell_m = 1.0;
    s.boundaries = [FieldBoundary::Sea; 4];
    let sea = s.sea.as_mut().unwrap();
    sea.level_relative_m = 0.05;
    sea.waves[0].wave_vector_rad_m = [std::f64::consts::TAU / 8.0, 0.0];
    sea.waves[0].angular_frequency_rad_s = (9.81 * std::f64::consts::TAU / 8.0).sqrt();
    let flow = advance(&gpu, &mut field, &s);
    assert_eq!(flow, Default::default());
    assert_eq!(field.checkpoint_complete(0.0).unwrap(), initial);
}
#[test]
fn sea_recipes_reconstruct_through_checkpoint_compaction_with_bounded_state() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("sea replay requires GPU");
    let mut cp = Arc::new(checkpoint().checked().unwrap());
    let mut recipe = FieldRecipe::new(cp.key(), vec![], cp.digest(), String::new());
    let mut resident = ResidentWater::new(&gpu).unwrap();
    let mut direct = GpuWater::new(&gpu.device, &gpu.queue, cp.size).unwrap();
    direct.upload_checkpoint(&cp).unwrap();
    let initial = volume(&cp);
    let mut entered = 0.0;
    let mut left = 0.0;
    for n in 0..70 {
        let mut s = step(2, 0.01);
        s.sea.as_mut().unwrap().waves[0].phase_at_start_rad -=
            wave().angular_frequency_rad_s * n as f64 * 0.02;
        let out = resident
            .advance(&cp, &recipe, &s, &[[0.5, 0.5], [15.5, 15.5]], false)
            .unwrap();
        let f = advance(&gpu, &mut direct, &s);
        entered += f64::from(f.entered);
        left += f64::from(f.left);
        let actual = direct.checkpoint_complete(0.0).unwrap();
        assert_eq!(actual.digest(), out.summary.digest);
        assert!((volume(&actual) - initial - entered + left).abs() < 2e-6);
        if let Some(seed) = out.checkpoint {
            assert_eq!(seed.as_raw(), &actual);
            cp = seed;
        }
        recipe = serde_json::from_slice(&serde_json::to_vec(&out.recipe).unwrap()).unwrap();
        let mut fresh = ResidentWater::new(&gpu).unwrap();
        let restored = fresh
            .observe(&cp, &recipe, &[[0.5, 0.5], [15.5, 15.5]])
            .unwrap();
        assert_eq!(restored.summary.digest, out.summary.digest);
        assert_eq!(restored.samples, out.samples);
    }
    assert_eq!(recipe.steps().len(), 6);
}

#[test]
fn a_dry_external_water_column_has_no_velocity_characteristic() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("dry sea interface requires GPU");
    let initial = checkpoint();
    let mut reference = None;
    for external in [0.0, 1000.0, -1000.0] {
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, initial.size).unwrap();
        field.upload_checkpoint(&initial).unwrap();
        let mut s = step(1, 0.001);
        s.sea = None;
        s.boundaries = [FieldBoundary::Radiation {
            surface_relative_m: -0.1,
            outward_velocity_mps: external,
        }; 4];
        let flow = advance(&gpu, &mut field, &s);
        assert!(flow.left > 0.0 && flow.entered == 0.0);
        let state = field.checkpoint_complete(0.0).unwrap();
        if let Some((cp, r)) = &reference {
            assert_eq!(&state, cp);
            assert_eq!(&flow, r);
        } else {
            reference = Some((state, flow));
        }
    }
}
