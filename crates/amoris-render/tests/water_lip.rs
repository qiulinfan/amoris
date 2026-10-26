//! The plunging lips' and falling sheets' finder (water.md 63.2): over a field's displayed columns,
//! the faces over which the resident kernel throws water become ribbons along the jet's parabola.
use amoris_render::{BackendChoice, Gpu, profiler::GpuProfiler, water_lip};
use std::sync::Arc;

fn read(gpu: &Gpu, buffer: &wgpu::Buffer, size: u64) -> Vec<u8> {
    let stage = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("water lips readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut e = gpu.device.create_command_encoder(&Default::default());
    e.copy_buffer_to_buffer(buffer, 0, &stage, 0, size);
    gpu.queue.submit([e.finish()]);
    stage
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let bytes = stage.slice(..).get_mapped_range().unwrap().to_vec();
    stage.unmap();
    bytes
}

/// The lips found over 16 x 16 columns of 0.5 m, laid out in rows: each column's surface, depth
/// and velocity along x and z.
fn lips(gpu: &Gpu, columns: &[[f32; 4]], spec: water_lip::FieldSpec) -> Vec<[f32; 12]> {
    found(gpu, columns, spec).0
}

/// The lips found, and each one's sheet as drawn: its left edge and velocity, its right edge and
/// velocity, and its side.
fn found(
    gpu: &Gpu,
    columns: &[[f32; 4]],
    spec: water_lip::FieldSpec,
) -> (Vec<[f32; 12]>, Vec<[f32; 20]>) {
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("columns"),
        size: (columns.len() * 16) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue
        .write_buffer(&buffer, 0, bytemuck::cast_slice(columns));
    let pipes = water_lip::Pipelines::new(&gpu.device);
    let pool = water_lip::Pool::new(&gpu.device, &gpu.queue, &pipes, &buffer);
    let mut profiler = GpuProfiler::new(&gpu.device, &gpu.queue, false, &gpu.info);
    let mut e = gpu.device.create_command_encoder(&Default::default());
    pool.encode(&gpu.queue, &mut e, &pipes, spec, &mut profiler);
    gpu.queue.submit([e.finish()]);
    let args: Vec<u32> = bytemuck::cast_slice(&read(gpu, &pool.args, 16)).to_vec();
    assert_eq!(args[0], water_lip::SEGMENTS * 6);
    let n = args[1] as usize;
    let all: Vec<f32> =
        bytemuck::cast_slice(&read(gpu, &pool.lips, water_lip::STRIDE * 64)).to_vec();
    let sheets: Vec<f32> =
        bytemuck::cast_slice(&read(gpu, &pool.sheets, water_lip::SHEET_STRIDE * 64)).to_vec();
    (
        (0..n.min(64))
            .map(|i| std::array::from_fn(|k| all[i * 12 + k]))
            .collect(),
        (0..n.min(64))
            .map(|i| std::array::from_fn(|k| sheets[i * 20 + k]))
            .collect(),
    )
}

#[test]
fn a_steep_fast_crest_throws_a_lip_ahead_and_still_water_none() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("lips need a GPU");
    gpu.device
        .on_uncaptured_error(Arc::new(|e| panic!("lips validation: {e}")));
    let spec = water_lip::FieldSpec {
        origin: [0.0, 0.0],
        size: [16, 16],
        cell: 0.5,
        tiled: false,
        falls_m: 0.0,
        plunging: true,
    };
    // A crest 2 m deep running along +x at 4 m/s, its front stepping down to 0.6 m between
    // columns 7 and 8: a slope of 2.8 and a Froude number of 0.9 (water.md 63.1). The face takes
    // the crest's speed, not the mean of it and the water at rest ahead.
    let front: Vec<[f32; 4]> = (0..256)
        .map(|n| {
            let i = n % 16;
            if i < 8 {
                [2.0, 2.0, 4.0, 0.0]
            } else {
                [0.6, 0.6, 0.0, 0.0]
            }
        })
        .collect();
    let found = lips(&gpu, &front, spec);
    assert_eq!(found.len(), 16, "one lip a row: {found:?}");
    for lip in &found {
        let [x, y, _, thickness, vx, _, _, flight, ..] = *lip;
        // Over the face between columns 7 and 8 (x = 4 m), from the crest's top layer: the share
        // 0.5 min(2.8 / 0.5 - 1, 1) min(0.9 / 0.5 - 1, 1) = 0.39 of its 2 m.
        assert!((x - 4.0).abs() < 1e-4, "{lip:?}");
        assert!((thickness - 0.79).abs() < 0.02, "{lip:?}");
        assert!((y - (2.0 - 0.5 * thickness)).abs() < 1e-4, "{lip:?}");
        // At the crest's long-wave speed, sqrt(9.81 x 2) = 4.43 m/s, until it falls to 0.6 m.
        assert!((vx - 4.43).abs() < 0.01, "{lip:?}");
        let fall = (2.0 * (y - 0.6) / 9.81).sqrt();
        assert!(
            (flight - fall).abs() < 0.5 * 0.5 / vx + 0.02,
            "{flight} against {fall}"
        );
    }
    // The same water at rest, and plunging off: nothing is thrown.
    let still: Vec<[f32; 4]> = front.iter().map(|c| [c[0], c[1], 0.0, 0.0]).collect();
    assert!(lips(&gpu, &still, spec).is_empty());
    let off = water_lip::FieldSpec {
        plunging: false,
        ..spec
    };
    assert!(lips(&gpu, &front, off).is_empty());
}

#[test]
fn the_ribbons_of_one_sheet_share_their_edges() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("lips need a GPU");
    gpu.device
        .on_uncaptured_error(Arc::new(|e| panic!("lips validation: {e}")));
    let spec = water_lip::FieldSpec {
        origin: [0.0, 0.0],
        size: [16, 16],
        cell: 0.5,
        tiled: false,
        falls_m: 0.0,
        plunging: true,
    };
    // The crest of the test above, faster in some rows than others: each row's lip flies its own
    // way, yet each meets the next at one edge, the mean of the two.
    let front: Vec<[f32; 4]> = (0..256)
        .map(|n| {
            let (i, row) = (n % 16, n / 16);
            if i < 8 {
                [2.0, 2.0, 4.0 + 0.5 * (row % 3) as f32, 0.0]
            } else {
                [0.6, 0.6, 0.0, 0.0]
            }
        })
        .collect();
    let (lips, sheets) = found(&gpu, &front, spec);
    assert_eq!(lips.len(), 16);
    // By row (z), from the lip's centre.
    let mut rows: Vec<([f32; 12], [f32; 20])> = lips.into_iter().zip(sheets).collect();
    rows.sort_by(|a, b| a.0[2].total_cmp(&b.0[2]));
    for pair in rows.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        // a's right edge is b's left, bit for bit, and lies between their launches.
        assert_eq!(a.1[8..16], b.1[0..8], "{a:?} {b:?}");
        assert!((a.1[10] - 0.5 * (a.0[2] + b.0[2])).abs() < 1e-6);
        assert!((a.1[15] - 0.5 * (a.0[7] + b.0[7])).abs() < 1e-6);
    }
    // At the sheet's ends, the end ribbons' own edges: half a column out from their centres.
    let (first, last) = (&rows[0], &rows[15]);
    assert!((first.1[2] - (first.0[2] - 0.25)).abs() < 1e-6, "{first:?}");
    assert_eq!(first.1[7], first.0[7]);
    assert!((last.1[10] - (last.0[2] + 0.25)).abs() < 1e-6, "{last:?}");
}
