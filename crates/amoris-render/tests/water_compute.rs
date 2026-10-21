//! Numerical checks on real GPUs, including WebGPU's baseline feature/limit configuration.
use amoris_render::water_compute::GpuWater;
use amoris_render::{BackendChoice, Gpu};
use std::future::Future;

fn volume(c: &[[f32; 4]]) -> f64 {
    c.iter().map(|c| f64::from(c[1])).sum()
}

#[test]
fn dropping_async_readback_releases_the_map_for_a_new_step() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("GPU water tests require a GPU");
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, [16, 16]).unwrap();
        let start: Vec<_> = (0..256)
            .map(|n| [0.0, if n % 16 < 8 { 2.0 } else { 0.0 }, 0.0, 0.0])
            .collect();
        let mut columns = start.clone();
        let active = vec![1; 256];
        let mut pending =
            Box::pin(field.advance_async(&mut columns, &active, 0.5, 1.0 / 120.0, 2, 0.02));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(pending.as_mut().poll(&mut context).is_pending());
        drop(pending);
        columns.clone_from(&start);
        field
            .advance(&mut columns, &active, 0.5, 1.0 / 120.0, 2, 0.02)
            .unwrap();
        let mut reference = GpuWater::new(&gpu.device, &gpu.queue, [16, 16]).unwrap();
        let mut expected = start;
        reference
            .advance(&mut expected, &active, 0.5, 1.0 / 120.0, 2, 0.02)
            .unwrap();
        assert_eq!(
            columns, expected,
            "cancelled work must not contaminate a fresh input"
        );
        assert!(pollster::block_on(scope.pop()).is_none());
    }
}

#[test]
fn gpu_water_preserves_lakes_transports_a_dam_and_respects_inactive_columns() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("GPU water tests require a GPU");
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let size = [32, 24];
        let n = (size[0] * size[1]) as usize;
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "pipeline validation: {error:?}");
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let on = vec![1; n];
        let mut lake: Vec<_> = (0..n)
            .map(|i| {
                let bed = (i % 9) as f32 * 0.125;
                [bed, 2.0 - bed, 0.0, 0.0]
            })
            .collect();
        let still = lake.clone();
        let result = field.advance(&mut lake, &on, 0.5, 1.0 / 120.0, 2, 0.02);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        result.unwrap();
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        assert_eq!(
            lake, still,
            "a lake at rest over an uneven bed stays at rest"
        );
        let dam: Vec<_> = (0..n)
            .map(|i| [0.0, if i % 32 < 16 { 2.0 } else { 0.0 }, 0.0, 0.0])
            .collect();
        let mut water = dam.clone();
        let initial = volume(&water);
        for _ in 0..120 {
            field
                .advance(&mut water, &on, 0.5, 1.0 / 240.0, 4, 0.02)
                .unwrap();
        }
        assert!(
            ((volume(&water) - initial) / initial).abs() < 2e-5,
            "closed dam conserves water"
        );
        assert!(water[22][1] > 0.05, "water reaches the dry side");
        assert!(
            water
                .iter()
                .all(|c| c[1] >= 0.0 && c.iter().all(|v| v.is_finite()))
        );
        assert!(
            water
                .iter()
                .enumerate()
                .all(|(n, c)| n % 32 != 0 || c[2] == 0.0)
        );
        // An already resident field advances without uploading the CPU result again. The output
        // array deliberately contains invalid input values: it must not be read as input.
        let mut expected = water.clone();
        field
            .advance(&mut expected, &on, 0.5, 1.0 / 240.0, 4, 0.02)
            .unwrap();
        field.upload(&water, &on).unwrap();
        let mut resident = vec![[f32::NAN; 4]; n];
        field
            .advance_uploaded(&mut resident, 0.5, 1.0 / 240.0, 4, 0.02)
            .unwrap();
        assert_eq!(
            resident, expected,
            "resident and explicitly uploaded steps agree bit for bit"
        );
        water = resident;
        let checkpoint = water.clone();
        for _ in 0..4 {
            field
                .advance(&mut water, &on, 0.5, 1.0 / 240.0, 4, 0.02)
                .unwrap();
        }
        let expected = water.clone();
        water = checkpoint;
        for _ in 0..4 {
            field
                .advance(&mut water, &on, 0.5, 1.0 / 240.0, 4, 0.02)
                .unwrap();
        }
        assert_eq!(
            water, expected,
            "restoring the same field repeats on the same device"
        );
        let mut mask = on.clone();
        for k in 0..24 {
            for i in 16..32 {
                mask[i + 32 * k] = 0;
            }
        }
        let mut water = dam.clone();
        for _ in 0..4 {
            field
                .advance(&mut water, &mask, 0.5, 1.0 / 120.0, 2, 0.02)
                .unwrap();
        }
        assert_eq!(
            water, dam,
            "the active boundary is a wall, inactive columns unchanged"
        );
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{backend:?}: {error:?}");
    }
}
