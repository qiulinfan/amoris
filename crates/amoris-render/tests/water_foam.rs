//! Numerical evidence for temporal foam, independent of the water lighting shader.
use amoris_render::water_foam::WaterFoam;
use amoris_render::{BackendChoice, Gpu};

const NX: usize = 32;
const NZ: usize = 16;
fn read(gpu: &Gpu, foam: &WaterFoam) -> Vec<f32> {
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("foam check"),
        size: (NX * NZ * 4 + 256 * NZ) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(foam.buffer(), 0, &buffer, 0, (NX * NZ * 4) as u64);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: foam.texture(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: (NX * NZ * 4) as u64,
                bytes_per_row: Some(256),
                rows_per_image: Some(NZ as u32),
            },
        },
        wgpu::Extent3d {
            width: NX as u32,
            height: NZ as u32,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let result = {
        let mapped = buffer.slice(..).get_mapped_range().unwrap();
        let values: Vec<f32> = bytemuck::cast_slice(&mapped[..NX * NZ * 4]).to_vec();
        for row in 0..NZ {
            let start = NX * NZ * 4 + row * 256;
            let texture: &[f32] = bytemuck::cast_slice(&mapped[start..start + NX * 4]);
            assert_eq!(
                &values[row * NX..(row + 1) * NX],
                texture,
                "the sampled density texture must match the advected state"
            );
        }
        values
    };
    buffer.unmap();
    result
}
fn step(gpu: &Gpu, foam: &mut WaterFoam, count: usize, dt: f32, clear: bool) {
    for _ in 0..count {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        foam.encode(&gpu.queue, &mut encoder, dt, clear, None);
        gpu.queue.submit([encoder.finish()]);
    }
}
fn moment(f: &[f32]) -> (f64, f64) {
    let total: f64 = f.iter().map(|&v| f64::from(v)).sum();
    let x: f64 = f
        .iter()
        .enumerate()
        .map(|(n, &v)| (n % NX) as f64 * f64::from(v))
        .sum();
    (total, x / total.max(1e-10))
}
#[test]
fn foam_is_carried_decays_clears_and_does_not_whiten_flat_fast_water() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("foam checks require a GPU");
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("foam water"),
            size: (NX * NZ * 16) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut foam = WaterFoam::new(&gpu.device, &buffer, [NX as u32, NZ as u32], 0.5, false);
        // A fast sheet over a ledge creates foam at the ledge, not everywhere it flows.
        let water: Vec<[f32; 4]> = (0..NX * NZ)
            .map(|n| [if n % NX < 6 { 2.5 } else { 1.0 }, 0.25, 6.0, 0.0])
            .collect();
        gpu.queue
            .write_buffer(&buffer, 0, bytemuck::cast_slice(&water));
        step(&gpu, &mut foam, 60, 1.0 / 60.0, false);
        let tiled_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tiled foam water"),
            size: (NX * NZ * 16) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut tiled_water = vec![[0.0f32; 4]; NX * NZ];
        for k in 0..NZ {
            for i in 0..NX {
                let at = ((i / 16) + (NX / 16) * (k / 16)) * 256 + i % 16 + 16 * (k % 16);
                tiled_water[at] = water[i + NX * k];
            }
        }
        gpu.queue
            .write_buffer(&tiled_buffer, 0, bytemuck::cast_slice(&tiled_water));
        let mut tiled = WaterFoam::new(
            &gpu.device,
            &tiled_buffer,
            [NX as u32, NZ as u32],
            0.5,
            true,
        );
        step(&gpu, &mut tiled, 60, 1.0 / 60.0, false);
        let seed = read(&gpu, &foam);
        assert_eq!(
            seed,
            read(&gpu, &tiled),
            "the feed tile layout matches a row-major field"
        );
        let peak = seed.iter().copied().fold(0.0, f32::max);
        eprintln!("{backend:?}: foam peak {peak}");
        assert!(
            peak > 0.15,
            "a moving sheet must produce a visible localized trail"
        );
        assert!(
            seed.iter()
                .all(|&v| v.is_finite() && (0.0..=1.0).contains(&v))
        );
        let (mass0, x0) = moment(&seed);
        // Remove the source and let the same foam drift one column to the right in half a second.
        let water = vec![[1.0f32, 2.0, 1.0, 0.0]; NX * NZ];
        gpu.queue
            .write_buffer(&buffer, 0, bytemuck::cast_slice(&water));
        step(&gpu, &mut foam, 30, 1.0 / 60.0, false);
        let drift = read(&gpu, &foam);
        let (mass1, x1) = moment(&drift);
        assert!(x1 > x0 + 0.85, "foam must follow the current: {x0} -> {x1}");
        let expected = mass0 * (-0.35f64 * 0.5).exp();
        assert!(
            (mass1 - expected).abs() / expected < 0.002,
            "decay: {mass1} vs {expected}"
        );
        // A missed rendering interval must still consume the elapsed lifetime, rather than
        // clamping time and leaving old foam almost unchanged after a long hitch.
        let calm = vec![[1.0f32, 2.0, 0.0, 0.0]; NX * NZ];
        gpu.queue
            .write_buffer(&buffer, 0, bytemuck::cast_slice(&calm));
        step(&gpu, &mut foam, 1, 5.0, false);
        let (after_gap, _) = moment(&read(&gpu, &foam));
        let expected_gap = mass1 * (-0.35f64 * 5.0).exp();
        assert!((after_gap - expected_gap).abs() / expected_gap < 0.002);
        step(&gpu, &mut foam, 1, 0.0, true);
        assert!(read(&gpu, &foam).iter().all(|&v| v == 0.0));
        // Neither deep nor shallow uniform fast water is itself a breaking wave.
        for depth in [2.0, 0.25] {
            let water = vec![[1.0f32, depth, 10.0, 0.0]; NX * NZ];
            gpu.queue
                .write_buffer(&buffer, 0, bytemuck::cast_slice(&water));
            step(&gpu, &mut foam, 10, 1.0 / 60.0, false);
            assert!(read(&gpu, &foam).iter().all(|&v| v == 0.0));
        }
        // An unreceived half is unknown data, not a cliff, compression or dry shore.
        let missing = (0..NX * NZ)
            .map(|n| {
                if n % NX < 16 {
                    [6.4f32, 0.4, 6.0, 0.0]
                } else {
                    [0.0, -1.0, 0.0, 0.0]
                }
            })
            .collect::<Vec<_>>();
        gpu.queue
            .write_buffer(&buffer, 0, bytemuck::cast_slice(&missing));
        step(&gpu, &mut foam, 1, 0.0, true);
        step(&gpu, &mut foam, 30, 1.0 / 60.0, false);
        assert!(
            read(&gpu, &foam).iter().all(|&v| v == 0.0),
            "unreceived tiles generated foam"
        );
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{backend:?}: {error:?}");
    }
}
