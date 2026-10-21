//! Actual flux receipts, independent volume checks and directional/corner boundary coverage.
use amoris_assets::water_field::{FieldBoundary, FieldCheckpoint};
use amoris_render::{BackendChoice, Gpu, water_compute::GpuWater};
fn volume(c: &FieldCheckpoint) -> f64 {
    c.columns
        .iter()
        .map(|c| f64::from(f32::from_bits(c[1])))
        .sum()
}
fn step(
    gpu: &Gpu,
    field: &mut GpuWater,
    edges: &[FieldBoundary; 4],
    cell: f32,
    friction: f32,
) -> (
    FieldCheckpoint,
    amoris_render::water_compute::BoundaryReport,
) {
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    field
        .encode_boundaries(&mut encoder, cell, 1.0 / 120.0, 2, friction, edges)
        .unwrap();
    gpu.queue.submit([encoder.finish()]);
    let report = field.read_boundary_report().unwrap();
    (field.checkpoint_complete(0.0).unwrap(), report)
}
#[test]
fn prescribed_flux_fills_dry_cells_on_each_side_and_both_corner_owners_survive() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("boundary tests require a GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("boundary GPU validation: {e}")
        }));
        for sides in [vec![0], vec![1], vec![2], vec![3], vec![0, 2], vec![1, 3]] {
            let cp = FieldCheckpoint::new([16, 16], 0.0, vec![[0; 4]; 256]);
            let mut field = GpuWater::new(&gpu.device, &gpu.queue, cp.size).unwrap();
            field.upload_checkpoint(&cp).unwrap();
            let mut edges = [FieldBoundary::Wall; 4];
            for &s in &sides {
                edges[s] = FieldBoundary::Inflow { m3s_per_m: 0.5 };
            }
            let mut entered = 0.0;
            let mut current = cp;
            for _ in 0..60 {
                let (next, r) = step(&gpu, &mut field, &edges, 1.0, 0.02);
                entered += f64::from(r.entered);
                assert_eq!(r.left, 0.0);
                current = next;
            }
            let expected = 0.5 * 16.0 * sides.len() as f64;
            assert!(
                (entered - expected).abs() < expected * 2e-6,
                "receipt {sides:?}: {entered} vs {expected}"
            );
            assert!(
                (volume(&current) - entered).abs() < entered * 2e-5,
                "volume {sides:?}"
            );
            assert!(
                current
                    .columns
                    .iter()
                    .all(|c| c.iter().all(|x| f32::from_bits(*x).is_finite())
                        && f32::from_bits(c[1]) >= 0.0)
            );
        }
    }
}
#[test]
fn uniform_balanced_river_has_matching_inlet_outlet_and_an_independent_mass_check() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("river test requires a GPU");
        gpu.device
            .on_uncaptured_error(std::sync::Arc::new(|e| panic!("river GPU validation: {e}")));
        let size = [32, 16];
        let slope = 0.2 / 9.81;
        let mut cp = FieldCheckpoint::new(
            size,
            0.0,
            (0..512)
                .map(|n| {
                    [
                        (-(n % 32) as f32 * slope).to_bits(),
                        1.0f32.to_bits(),
                        1.0f32.to_bits(),
                        0,
                    ]
                })
                .collect(),
        );
        cp.far_faces[..16].fill(1.0f32.to_bits());
        let start = volume(&cp);
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
        field.upload_checkpoint(&cp).unwrap();
        let edges = [
            FieldBoundary::Inflow { m3s_per_m: 1.0 },
            FieldBoundary::Outflow,
            FieldBoundary::Wall,
            FieldBoundary::Wall,
        ];
        let (mut entered, mut left) = (0.0, 0.0);
        for _ in 0..120 {
            let (next, r) = step(&gpu, &mut field, &edges, 1.0, 0.1);
            cp = next;
            entered += f64::from(r.entered);
            left += f64::from(r.left);
        }
        eprintln!(
            "balanced river: entered {entered}, left {left}, mass {}",
            volume(&cp)
        );
        assert!((entered - 32.0).abs() < 1e-4);
        assert!((left - entered).abs() < 0.01);
        assert!((volume(&cp) - start - entered + left).abs() < 0.005);
        assert!(
            cp.columns
                .iter()
                .all(|c| (f32::from_bits(c[1]) - 1.0).abs() < 0.001)
        );
    }
}

#[test]
fn draining_is_limited_to_existing_water_and_closure_drops_old_receipts() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("drain requires a GPU");
        gpu.device
            .on_uncaptured_error(std::sync::Arc::new(|e| panic!("drain validation: {e}")));
        let mut cp = FieldCheckpoint::new(
            [16, 16],
            0.0,
            (0..256)
                .map(|n| {
                    [
                        0,
                        0.25f32.to_bits(),
                        (((n * 17) % 201) as f32 - 100.0).to_bits(),
                        (((n * 31) % 201) as f32 - 100.0).to_bits(),
                    ]
                })
                .collect(),
        );
        cp.far_faces.fill(100.0f32.to_bits());
        let start = volume(&cp);
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, cp.size).unwrap();
        field.upload_checkpoint(&cp).unwrap();
        let edges = [FieldBoundary::Outflow; 4];
        let mut left = 0.0;
        for _ in 0..120 {
            let (next, r) = step(&gpu, &mut field, &edges, 1.0, 0.02);
            assert_eq!(r.entered, 0.0);
            left += f64::from(r.left);
            cp = next;
        }
        assert!(left > 0.0 && left <= start + 0.001);
        assert!((volume(&cp) + left - start).abs() < 0.002);
        assert!(cp.columns.iter().all(|c| f32::from_bits(c[1]) >= 0.0));
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        field
            .encode_boundaries(
                &mut encoder,
                1.0,
                1.0 / 120.0,
                2,
                0.02,
                &[FieldBoundary::Wall; 4],
            )
            .unwrap();
        gpu.queue.submit([encoder.finish()]);
        let closed = field.checkpoint_complete(0.0).unwrap();
        assert!(closed.far_faces.iter().all(|v| *v == 0));
        assert!(field.read_boundary_report().is_err());
    }
}

#[test]
fn local_edit_receipts_measure_stored_clamped_changes_before_transport() {
    use amoris_assets::water_field::{ColumnDelta, FieldRecipe, FieldStep};
    use amoris_render::water_resident::ResidentWater;
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("edit receipt requires a GPU");
        gpu.device
            .on_uncaptured_error(std::sync::Arc::new(|e| panic!("edit validation: {e}")));
        let cp = std::sync::Arc::new(
            FieldCheckpoint::new([16, 16], 0.0, vec![[0, 1.0f32.to_bits(), 0, 0]; 256])
                .checked()
                .unwrap(),
        );
        let before = FieldRecipe::new(cp.key(), vec![], cp.digest(), String::new());
        let mut resident = ResidentWater::new(&gpu).unwrap();
        let step = FieldStep {
            sea: None,
            boundaries: [FieldBoundary::Wall; 4],
            cell_m: 1.0,
            substep_dt: 1.0 / 120.0,
            substeps: 2,
            friction: 0.02,
            dry_below: 0.0,
            impulses: vec![],
            edits: vec![
                ColumnDelta {
                    column: 0,
                    depth_m: 0.25,
                },
                ColumnDelta {
                    column: 0,
                    depth_m: -10.0,
                },
                ColumnDelta {
                    column: 1,
                    depth_m: 0.5,
                },
                ColumnDelta {
                    column: 1,
                    depth_m: -0.2,
                },
            ],
        };
        let out = resident.advance(&cp, &before, &step, &[], true).unwrap();
        assert!((out.boundary.edited + 0.7).abs() < 1e-6);
        assert_eq!(out.boundary.entered, 0.0);
        assert_eq!(out.boundary.left, 0.0);
        assert!((volume(out.checkpoint.as_ref().unwrap()) - 255.3).abs() < 0.0001);
    }
}

#[test]
fn receiving_stage_holds_a_lake_and_lowering_it_drains_each_side_with_a_measured_budget() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("receiving stage requires a GPU");
        gpu.device
            .on_uncaptured_error(std::sync::Arc::new(|e| panic!("stage validation: {e}")));
        for side in 0..4 {
            let cp = FieldCheckpoint::new([16, 16], 0.0, vec![[0, 1.0f32.to_bits(), 0, 0]; 256]);
            let mut field = GpuWater::new(&gpu.device, &gpu.queue, cp.size).unwrap();
            field.upload_checkpoint(&cp).unwrap();
            let mut edges = [FieldBoundary::Wall; 4];
            edges[side] = FieldBoundary::Stage {
                surface_relative_m: 1.0,
            };
            let (same, r) = step(&gpu, &mut field, &edges, 1.0, 0.02);
            assert!(
                same == cp,
                "a receiving stage equal to a still surface must preserve exact words"
            );
            assert_eq!(r.entered, 0.0);
            assert_eq!(r.left, 0.0);
            edges[side] = FieldBoundary::Stage {
                surface_relative_m: 0.0,
            };
            let mut left = 0.0;
            let mut current = cp;
            for _ in 0..60 {
                let (next, r) = step(&gpu, &mut field, &edges, 1.0, 0.02);
                assert_eq!(r.entered, 0.0);
                left += f64::from(r.left);
                current = next;
            }
            assert!(left > 1.0);
            assert!((volume(&current) + left - 256.0).abs() < 0.002);
        }
    }
}

#[test]
fn a_closed_reupload_cannot_consume_old_open_boundary_scratch() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("boundary scratch test needs GPU");
        gpu.device
            .on_uncaptured_error(std::sync::Arc::new(|e| panic!("boundary scratch: {e}")));
        for side in 0..4 {
            let axis = usize::from(side >= 2);
            let outward = if side % 2 == 0 { -3.0f32 } else { 3.0 };
            let mut open =
                FieldCheckpoint::new([16, 16], 0.0, vec![[0, 2.0f32.to_bits(), 0, 0]; 256]);
            for c in &mut open.columns {
                c[axis + 2] = outward.to_bits();
            }
            if axis == 0 {
                open.far_faces[..16].fill(outward.to_bits());
            } else {
                open.far_faces[16..].fill(outward.to_bits());
            }
            let mut warm = GpuWater::new(&gpu.device, &gpu.queue, open.size).unwrap();
            warm.upload_checkpoint(&open).unwrap();
            let mut edges = [FieldBoundary::Wall; 4];
            edges[side] = FieldBoundary::Outflow;
            let (_, r) = step(&gpu, &mut warm, &edges, 0.25, 0.0);
            assert!(r.left > 0.01);
            let closed = FieldCheckpoint::new(
                [16, 16],
                0.0,
                (0..256)
                    .map(|n| {
                        let mut c = [0, 0.001f32.to_bits(), 0, 0];
                        if (axis == 0 && n % 16 > 0) || (axis == 1 && n >= 16) {
                            c[axis + 2] = (-outward).to_bits();
                        }
                        c
                    })
                    .collect(),
            );
            warm.upload_checkpoint(&closed).unwrap();
            let mut fresh = GpuWater::new(&gpu.device, &gpu.queue, closed.size).unwrap();
            fresh.upload_checkpoint(&closed).unwrap();
            let closed_step = |field: &mut GpuWater| {
                let mut e = gpu.device.create_command_encoder(&Default::default());
                field
                    .encode_maccormack(&mut e, 0.25, 1.0 / 120.0, 2, 0.0)
                    .unwrap();
                gpu.queue.submit([e.finish()]);
                field.checkpoint_complete(0.0).unwrap()
            };
            let a = closed_step(&mut warm);
            let b = closed_step(&mut fresh);
            assert!(
                a == b,
                "side {side}: old open boundary scratch changed a closed, reuploaded field"
            );
        }
    }
}
