#[path = "common/water_advection.rs"]
mod experiment;
use amoris_render::water_compute::GpuWater;
use amoris_render::{BackendChoice, Gpu};

#[test]
fn bounded_maccormack_reduces_error_in_an_analytically_translating_shear() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("advection requires a GPU");
        gpu.device
            .on_uncaptured_error(std::sync::Arc::new(|e| panic!("advection validation: {e}")));
        let first = experiment::shear(&gpu, 128, 1.0, false, 120).unwrap();
        let modern = experiment::shear(&gpu, 128, 1.0, true, 120).unwrap();
        eprintln!(
            "{}",
            serde_json::json!({"backend":format!("{:?}",backend),"first":first,"modern":modern})
        );
        // These bound accuracy, not performance. Both use the same analytic damped shear.
        assert!(
            modern.relative_l2 < first.relative_l2 * 0.7,
            "{first:?}\n{modern:?}"
        );
        for result in [&first, &modern] {
            let peak = result
                .profile
                .iter()
                .map(|p| p[2].abs())
                .fold(0.0, f64::max);
            assert!(peak <= result.expected_amplitude + 1e-4, "{result:?}");
            assert!(result.roi_depth_error < 1e-4, "{result:?}");
            assert!(result.roi_cross_velocity_error < 1e-4, "{result:?}");
            assert!(result.relative_mass_error.abs() < 1e-6, "{result:?}");
        }
    }
}

#[test]
fn modern_transport_keeps_lakes_at_rest_and_conserves_water_around_dry_and_frozen_cells() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("modern wet/dry requires a GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("modern wet/dry validation: {e}")
        }));
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, [32, 32]).unwrap();
        let lake: Vec<_> = (0..1024)
            .map(|n| {
                let bed = if n % 32 >= 24 {
                    3.0
                } else {
                    (n % 8) as f32 * 0.25
                };
                [bed, (2.0f32 - bed).max(0.0), 0.0, 0.0]
            })
            .collect();
        let mut actual = lake.clone();
        field
            .advance_maccormack(&mut actual, &[1; 1024], 0.5, 1.0 / 240.0, 4, 0.02)
            .unwrap();
        assert_eq!(
            actual, lake,
            "a still lake over a stepped bed must remain exact"
        );
        let initial: Vec<_> = (0..1024)
            .map(|n| {
                if n / 32 >= 28 {
                    return [
                        0.0,
                        1.5,
                        if n % 32 == 0 { 0.0 } else { 0.3 },
                        if n / 32 == 28 { 0.0 } else { -0.2 },
                    ];
                }
                if (24..27).contains(&(n % 32)) {
                    return [3.0, 0.0, 0.0, 0.0];
                }
                let wet = n % 32 < 16;
                [
                    0.0,
                    if wet { 2.0 } else { 0.0 },
                    if wet && n % 32 > 0 { 0.3 } else { 0.0 },
                    0.0,
                ]
            })
            .collect();
        let active: Vec<_> = (0..1024).map(|n| u32::from(n / 32 < 28)).collect();
        let volume = |a: &[[f32; 4]]| a.iter().map(|c| f64::from(c[1])).sum::<f64>();
        actual.clone_from(&initial);
        for _ in 0..100 {
            field
                .advance_maccormack(&mut actual, &active, 0.5, 1.0 / 240.0, 2, 0.02)
                .unwrap();
        }
        assert!((volume(&actual) - volume(&initial)).abs() / volume(&initial) < 2e-5);
        assert!(
            actual
                .iter()
                .all(|c| c[1] >= 0.0 && c.iter().all(|v| v.is_finite()))
        );
        for (n, (old, new)) in initial.iter().zip(&actual).enumerate() {
            assert_eq!(old[0].to_bits(), new[0].to_bits(), "bed at {n}");
            if active[n] == 0 {
                assert_eq!(
                    old.map(f32::to_bits),
                    new.map(f32::to_bits),
                    "frozen column {n}"
                );
            }
            if old[0] == 3.0 {
                assert_eq!(new[1], 0.0, "unreached dry obstacle {n}");
            }
        }
        assert!(
            actual[20 + 32 * 16][1] > 0.01,
            "dam flow must actually reach the initially dry side"
        );
    }
}

#[test]
fn resizing_a_modern_window_reuses_pipelines_and_grows_its_scratch_safely() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("modern resize requires a GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("modern resize validation: {e}")
        }));
        let mut reused = GpuWater::new(&gpu.device, &gpu.queue, [64, 64]).unwrap();
        for size in [[16, 16], [32, 32], [16, 32], [64, 64]] {
            reused.set_size(size).unwrap();
            let initial: Vec<_> = (0..size[0] * size[1])
                .map(|n| {
                    [
                        0.0,
                        if n % size[0] < size[0] / 2 { 2.0 } else { 0.0 },
                        0.0,
                        0.0,
                    ]
                })
                .collect();
            let mask = vec![1; initial.len()];
            let mut actual = initial.clone();
            let mut expected = initial;
            let mut fresh = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
            reused
                .advance_maccormack(&mut actual, &mask, 0.5, 1.0 / 120.0, 2, 0.02)
                .unwrap();
            fresh
                .advance_maccormack(&mut expected, &mask, 0.5, 1.0 / 120.0, 2, 0.02)
                .unwrap();
            assert_eq!(actual, expected, "{size:?}");
        }
    }
}
