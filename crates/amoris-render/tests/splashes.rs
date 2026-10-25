//! The splashes' solver (docs/spec/splashes.md 4, 8): in a region whose mask has a solid floor and
//! a liquid block, a particle dropped on the floor rests on it, one dropped into the liquid dies,
//! one thrown out of the region dies, the resting one dies at its lifetime, and a full ring gives
//! its oldest slots to new particles. Skips when there is no GPU.

use amoris_render::fluid::{Fluid, FluidTuning, Region, SOLID, SolverSpec};
use amoris_render::{BackendChoice, Gpu};

const SIZE: u64 = 80;
const DEAD: u32 = u32::MAX;

fn gpu() -> Option<Gpu> {
    let gpu = Gpu::headless(BackendChoice::from_env()).ok();
    if gpu.is_none() {
        eprintln!("no GPU: skipped");
    }
    gpu
}

/// Each particle's position (world metres) and tag.
fn particles(gpu: &Gpu, f: &Fluid) -> Vec<([f32; 3], u32)> {
    let src = f.particle_buffer();
    let read = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("particles readback"),
        size: src.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(src, 0, &read, 0, src.size());
    gpu.queue.submit([enc.finish()]);
    let slice = read.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv()
        .expect("a mapping answer")
        .expect("the buffer maps");
    let bytes = slice.get_mapped_range().expect("mapped").to_vec();
    read.unmap();
    let words: Vec<u32> = bytemuck::pod_collect_to_vec(&bytes);
    let per = (SIZE / 4) as usize;
    let cell = f.region(0).map_or(0.25, |r| r.cell_m);
    words
        .chunks(per)
        .map(|p| {
            let x = [0, 1, 2].map(|a| f32::from_bits(p[a]) * cell);
            (x, p[7])
        })
        .collect()
}

fn step(gpu: &Gpu, f: &mut Fluid, frames: u32) {
    for _ in 0..frames {
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        f.step(&gpu.queue, &mut enc, 1.0 / 60.0, None);
        gpu.queue.submit([enc.finish()]);
    }
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
}

#[test]
fn particles_rest_on_solids_die_in_liquid_and_out_of_reach_and_the_ring_wraps() {
    let Some(gpu) = gpu() else { return };
    // 4 x 4 x 4 blocks of 1 m at 4 cells a block; the corner at the origin.
    let spec = SolverSpec {
        dims: [16; 3],
        per_block: 4,
        capacity: 8,
        substeps: 4,
        lifetime_s: 4.0,
        spacing: 0.5,
        tuning: FluidTuning::default(),
    };
    let mut f = Fluid::new(&gpu.device, spec);
    assert!(!f.active(), "nothing spawned: every pass is skipped");
    assert!(!f.encodes(1.0 / 60.0), "an idle solver takes no timestamps");
    // A solid floor (blocks y = 0) and a liquid block at (2, 1, 1), its surface at 8/9.
    let mut mask = vec![0u8; 64];
    for z in 0..4 {
        for x in 0..4 {
            mask[x + 16 * z] = SOLID;
        }
    }
    mask[2 + 4 + 16] = 8;
    f.open(
        &gpu.queue,
        0,
        Region {
            origin: [0.0; 3],
            cell_m: 0.25,
            absorb: [6.0, 1.8, 0.9],
        },
        &mask,
    );
    f.spawn(
        &gpu.queue,
        0,
        &[
            // Over the floor, into the liquid, thrown out along +x.
            ([1.5, 2.5, 1.5], [0.0; 3]),
            ([2.5, 2.5, 1.5], [0.0; 3]),
            ([1.5, 1.5, 2.5], [5.0, 0.0, 0.0]),
        ],
    );
    assert!(f.active());
    assert!(f.encodes(1.0 / 60.0));
    assert!(!f.encodes(0.0), "a frame of no time encodes no pass");
    step(&gpu, &mut f, 60);
    let p = particles(&gpu, &f);
    let (rest, tag) = p[0];
    assert_ne!(tag, DEAD, "the particle on the floor lives");
    assert!(
        // Within a solver cell of the floor: the cells beside a solid keep no velocity into it.
        (1.0..1.25).contains(&rest[1]) && (rest[0] - 1.5).abs() < 0.05,
        "it rests on the floor: {rest:?}"
    );
    assert_eq!(
        p[1].1, DEAD,
        "the particle in the liquid died: {:?}",
        p[1].0
    );
    assert_eq!(
        p[2].1, DEAD,
        "the particle out of the region died: {:?}",
        p[2].0
    );
    step(&gpu, &mut f, 190);
    assert_eq!(particles(&gpu, &f)[0].1, DEAD, "at 4 s it dies");
    assert!(!f.active(), "no particle can be alive now");
    assert!(
        !f.encodes(1.0 / 60.0),
        "after a splash its solver takes no timestamps"
    );

    // The ring: three in slots 3 to 5, then seven more take 6, 7, 0 to 4; slot 5 keeps its own.
    let at = |i: usize| ([0.6 + 0.1 * i as f32, 3.0, 2.0], [0.0; 3]);
    f.spawn(&gpu.queue, 0, &(0..3).map(at).collect::<Vec<_>>());
    f.spawn(&gpu.queue, 0, &(10..17).map(at).collect::<Vec<_>>());
    let p = particles(&gpu, &f);
    let xs: Vec<f32> = p
        .iter()
        .map(|(x, _)| (x[0] * 10.0).round() / 10.0)
        .collect();
    assert_eq!(xs, vec![1.8, 1.9, 2.0, 2.1, 2.2, 0.8, 1.6, 1.7]);
}
