//! Presentation spray: physical field sources, analytic motion, water reentry and bounded work.
use amoris_render::{
    Antialiasing, BackendChoice, Gpu, Gtao, SceneFormat,
    profiler::GpuProfiler,
    water_foam::WaterFoam,
    water_spray::{Advance, Entering, FieldSpec, PER_FRAME, Pipelines, Pool, SPLASH_PER_FRAME},
};
// A lost device or a validation error fails the case with its cause. The cases run in parallel:
// the harnesses' profilers keep no timestamps, and the occlusion case's renderer, the only one,
// holds one of the 32 timestamp query sets Metal gives a process (profiler.rs).
fn gpu(backend: BackendChoice, what: &'static str) -> Gpu {
    let g = Gpu::headless(backend).unwrap_or_else(|e| panic!("{what} needs a GPU: {e:?}"));
    g.device.set_device_lost_callback(move |reason, message| {
        if reason != wgpu::DeviceLostReason::Destroyed {
            panic!("{what}: device lost: {reason:?}: {message}");
        }
    });
    g.device.on_uncaptured_error(std::sync::Arc::new(move |e| {
        panic!("{what}: GPU validation failed: {e}");
    }));
    g
}
struct Harness {
    columns: wgpu::Buffer,
    view: wgpu::Buffer,
    pipes: Pipelines,
    pool: Pool,
    foam: WaterFoam,
    spec: FieldSpec,
    profiler: GpuProfiler,
    /// The stirring points going in at the next advance (water.md 52).
    entering: Vec<Entering>,
    /// Tally what the sources sound like from the camera (water_acoustics.rs).
    listen: bool,
}
fn read(g: &Gpu, b: &wgpu::Buffer) -> Vec<u32> {
    let stage = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("spray test readback"),
        size: b.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut e = g.device.create_command_encoder(&Default::default());
    e.copy_buffer_to_buffer(b, 0, &stage, 0, b.size());
    g.queue.submit([e.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    stage.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    g.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let values = bytemuck::cast_slice(&stage.slice(..).get_mapped_range().unwrap()).to_vec();
    stage.unmap();
    values
}
impl Harness {
    fn new(g: &Gpu, columns: &[[f32; 4]]) -> Self {
        Self::with_layout(g, columns, false)
    }
    fn with_layout(g: &Gpu, columns: &[[f32; 4]], tiled: bool) -> Self {
        let spec = FieldSpec {
            origin: [0.0; 2],
            size: [32, 16],
            cell: 0.5,
            tiled,
            absorb: [1.0, 0.2, 0.1],
        };
        let field = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spray test water"),
            size: columns.len() as u64 * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        g.queue
            .write_buffer(&field, 0, bytemuck::cast_slice(columns));
        let view = g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spray test view"),
            size: 1024,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipes = Pipelines::new(&g.device, SceneFormat::new(Antialiasing::Off, Gtao::Off));
        let foam = WaterFoam::new(&g.device, &field, spec.size, spec.cell, tiled);
        let pool = Pool::new(&g.device, &pipes, &view, &field, foam.impacts());
        Self {
            columns: field,
            view,
            pipes,
            pool,
            foam,
            spec,
            profiler: GpuProfiler::new(&g.device, &g.queue, false, &g.info),
            entering: Vec::new(),
            listen: false,
        }
    }
    fn step(&mut self, g: &Gpu, dt: f32, changed: bool, clear: bool) {
        let mut e = g.device.create_command_encoder(&Default::default());
        e.clear_buffer(&self.pipes.budget, 0, None);
        e.clear_buffer(self.foam.impacts(), 0, None);
        e.clear_buffer(&self.pipes.acoustics, 0, None);
        self.pool.encode(
            &g.queue,
            &mut e,
            &self.pipes,
            Advance {
                field: self.spec,
                dt,
                changed,
                clear,
                entering: &std::mem::take(&mut self.entering),
                listen: self.listen,
            },
            &mut self.profiler,
        );
        self.foam
            .encode(&g.queue, &mut e, dt.min(0.25), clear, true, 0.0, None);
        g.queue.submit([e.finish()]);
    }
    fn upload(&self, g: &Gpu, columns: &[[f32; 4]]) {
        g.queue
            .write_buffer(&self.columns, 0, bytemuck::cast_slice(columns));
    }
    fn seed(&self, g: &Gpu, kind: u32, pos: [f32; 3], vel: [f32; 3]) {
        let mut words = [0u32; 12];
        for i in 0..3 {
            words[i] = pos[i].to_bits();
            words[4 + i] = vel[i].to_bits();
        }
        words[7] = 4.0f32.to_bits();
        words[8] = kind;
        words[9] = 17;
        g.queue
            .write_buffer(&self.pool.particles, 0, bytemuck::cast_slice(&words));
    }
}
fn flat(depth: f32) -> Vec<[f32; 4]> {
    vec![[1.0, depth, 0.0, 0.0]; 512]
}
fn fall(axis: usize, reverse: bool) -> Vec<[f32; 4]> {
    (0..512)
        .map(|n| {
            let q = if axis == 0 { n % 32 } else { n / 32 };
            let split = if axis == 0 { 16 } else { 8 };
            let high = (q < split) != reverse;
            let mut c = [if high { 6.4 } else { 0.4 }, 0.4, 0.0, 0.0];
            c[axis + 2] = if reverse { -1.0 } else { 1.0 };
            c
        })
        .collect()
}
#[test]
fn field_sources_follow_four_flow_directions_and_stop_during_pause_and_after_flow() {
    for backend in BackendChoice::under_test() {
        let g = gpu(backend, "spray");
        let mut still = Harness::new(&g, &flat(1.0));
        for n in 0..5 {
            still.step(&g, 1.0 / 60.0, n == 0, n == 0);
        }
        assert_eq!(read(&g, &still.pool.state)[1], 0);
        assert_eq!(read(&g, &still.pool.state)[5], 0);
        for axis in 0..2 {
            for reverse in [false, true] {
                let mut missing = fall(axis, reverse);
                for c in &mut missing {
                    if c[0] < 1.0 {
                        *c = [0.0, -1.0, 0.0, 0.0];
                    }
                }
                let mut partial = Harness::new(&g, &missing);
                partial.step(&g, 1.0 / 60.0, true, true);
                let state = read(&g, &partial.pool.state);
                assert_eq!(state[3], 0, "missing tile became a spray source");
                assert_eq!(state[1], 0);
                let mut h = Harness::new(&g, &fall(axis, reverse));
                for n in 0..30 {
                    h.step(&g, 1.0 / 60.0, n == 0, n == 0);
                }
                let state = read(&g, &h.pool.state);
                assert!(state[0] > 100 && state[0] < 2000);
                let before = read(&g, &h.pool.particles);
                for p in before
                    .as_chunks::<12>()
                    .0
                    .iter()
                    .filter(|p| f32::from_bits(p[7]) > 0.0)
                {
                    assert_eq!(p[8], 0);
                    assert!(
                        f32::from_bits(p[4 + axis * 2]) * (if reverse { -1.0 } else { 1.0 }) > 0.0
                    );
                }
                for _ in 0..5 {
                    h.step(&g, 0.0, false, false);
                }
                assert!(
                    before == read(&g, &h.pool.particles),
                    "paused spray changed particles"
                );
                assert_eq!(read(&g, &h.pool.state)[0], state[0]);
                let mut stopped = fall(axis, reverse);
                for c in &mut stopped {
                    c[2] = 0.0;
                    c[3] = 0.0;
                }
                h.upload(&g, &stopped);
                h.step(&g, 1.0 / 60.0, true, false);
                assert_eq!(read(&g, &h.pool.state)[1], 0);
                let head = read(&g, &h.pool.state)[0];
                for _ in 0..360 {
                    h.step(&g, 1.0 / 60.0, false, false);
                }
                let state = read(&g, &h.pool.state);
                assert_eq!(state[0], head);
                assert_eq!(state[5], 0);
                assert_eq!(state[3], 0);
            }
        }
    }
}
#[test]
fn droplets_follow_analytic_ballistics_reenter_water_feed_foam_and_clear_history() {
    for backend in BackendChoice::under_test() {
        let g = gpu(backend, "spray motion");
        let mut h = Harness::new(&g, &flat(1.0));
        h.step(&g, 0.0, true, true);
        h.seed(&g, 2, [4.0, 10.0, 4.0], [1.0, 2.0, -0.5]);
        for _ in 0..10 {
            h.step(&g, 0.1, false, false);
        }
        let p = read(&g, &h.pool.particles);
        for (i, want) in [(0, 5.0), (1, 7.095), (2, 3.5), (5, -7.81)] {
            assert!(
                (f64::from(f32::from_bits(p[i])) - want).abs() < 3e-5,
                "analytic trajectory component {i}"
            );
        }
        h.step(&g, 0.0, false, true);
        h.seed(&g, 0, [4.0, 1.01, 4.0], [0.0, -6.0, 0.0]);
        h.step(&g, 1.0 / 60.0, false, false);
        let p = read(&g, &h.pool.particles);
        assert_eq!(p[8], 1);
        assert!(f32::from_bits(p[5]) > 2.0);
        assert!(
            read(&g, h.foam.buffer())
                .iter()
                .any(|w| f32::from_bits(*w) > 0.01)
        );
        for _ in 0..120 {
            h.step(&g, 1.0 / 60.0, false, false);
        }
        assert_eq!(read(&g, &h.pool.state)[5], 0);
        h.seed(&g, 1, [4.0, 3.0, 4.0], [0.0, 0.0, 0.0]);
        h.step(&g, 0.5, false, false);
        assert_eq!(read(&g, &h.pool.state)[5], 0);
        assert!(read(&g, &h.pool.particles).iter().all(|w| *w == 0));
        h.seed(&g, 0, [4.0, 1.01, 4.0], [0.0, -6.0, 0.0]);
        h.upload(&g, &flat(0.0));
        h.step(&g, 1.0 / 60.0, true, false);
        assert_eq!(read(&g, &h.pool.state)[5], 0);
        assert!(read(&g, h.foam.impacts()).iter().all(|w| *w == 0));
    }
}
#[test]
fn global_birth_budget_is_shared_across_water_regions() {
    let g = gpu(BackendChoice::from_env(), "spray budget");
    let field = (0..512)
        .map(|n| {
            [
                if (n % 32 + n / 32) % 2 == 0 { 6.4 } else { 0.4 },
                0.4,
                20.0,
                20.0,
            ]
        })
        .collect::<Vec<_>>();
    let mut h = Harness::new(&g, &field);
    // Both pools share the same global allocator, exactly as different renderer water regions do.
    let view = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("budget view"),
        size: 1024,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });
    let mut other = Pool::new(&g.device, &h.pipes, &view, &h.columns, h.foam.impacts());
    let mut e = g.device.create_command_encoder(&Default::default());
    e.clear_buffer(&h.pipes.budget, 0, None);
    h.pool.encode(
        &g.queue,
        &mut e,
        &h.pipes,
        Advance {
            field: h.spec,
            dt: 0.1,
            changed: true,
            clear: true,
            entering: &[],
            listen: false,
        },
        &mut h.profiler,
    );
    other.encode(
        &g.queue,
        &mut e,
        &h.pipes,
        Advance {
            field: h.spec,
            dt: 0.1,
            changed: true,
            clear: true,
            entering: &[],
            listen: false,
        },
        &mut h.profiler,
    );
    g.queue.submit([e.finish()]);
    let a = read(&g, &h.pool.state);
    let b = read(&g, &other.state);
    assert_eq!(a[1] + b[1], PER_FRAME);
    assert!(a[0] <= PER_FRAME && b[0] <= PER_FRAME);
}

#[test]
fn white_water_splashes_take_a_bounded_share_of_births() {
    let g = gpu(BackendChoice::from_env(), "splash budget");
    // Level water a metre deep converging on the middle at 4 m/s per metre: its surface rises at
    // about 4 m/s in every column, a white-water burst everywhere and no fall or rapid face.
    let field = (0..512)
        .map(|n| [1.0, 1.0, -4.0 * ((n % 32) as f32 * 0.5 + 0.25 - 8.0), 0.0])
        .collect::<Vec<_>>();
    let mut h = Harness::new(&g, &field);
    for _ in 0..2 {
        h.step(&g, 0.1, false, false);
        // Unbounded, these columns would ask for about 3,500 drops a frame.
        assert_eq!(read(&g, &h.pool.state)[1], SPLASH_PER_FRAME);
    }
}

#[test]
fn a_bore_throws_no_falling_sheet() {
    let g = gpu(BackendChoice::from_env(), "bore spray");
    // One flat bed: 2 m of water running at 3 m/s into 0.5 m of still water, a 1.5 m front.
    let field = (0..512)
        .map(|n| {
            if n % 32 < 16 {
                [2.0, 2.0, 3.0, 0.0]
            } else {
                [0.5, 0.5, 0.0, 0.0]
            }
        })
        .collect::<Vec<_>>();
    let mut h = Harness::new(&g, &field);
    for frame in 0..3 {
        h.step(&g, 1.0 / 60.0, true, frame == 0);
    }
    let sheet = read(&g, &h.pool.particles)
        .chunks(12)
        .filter(|d| f32::from_bits(d[7]) > f32::from_bits(d[3]) && d[8] == 0)
        .count();
    assert_eq!(sheet, 0, "a bore's front is not a waterfall");
}

#[test]
fn active_field_spray_is_occluded_by_the_opaque_scene() {
    use amoris_assets::frame::{InstanceUpdate, Look, Pose, RenderFrame};
    use amoris_render::{CameraState, Renderer};
    use glam::Vec3;
    let g = gpu(BackendChoice::from_env(), "spray occlusion");
    let mut images = Vec::new();
    for enabled in [false, true] {
        let mut r = Renderer::new(&g, wgpu::TextureFormat::Rgba8UnormSrgb, 320, 180);
        r.set_antialiasing(Antialiasing::Msaa);
        r.set_gtao(Gtao::Off);
        r.set_splashes(enabled);
        r.set_camera_override(Some(CameraState::look_at(
            Vec3::new(25.0, 5.0, 24.0),
            Vec3::new(14.0, 1.4, 16.0),
        )));
        let mut scene = amoris_render::demo::water::scene("fall");
        scene.instances.push(InstanceUpdate {
            id: 9999,
            pose: Some(Pose {
                position: [24.0, 5.0, 16.0],
                scale: [0.2, 50.0, 50.0],
                ..Default::default()
            }),
            look: Some(Look {
                mesh: "cube".into(),
                color: [0.1, 0.1, 0.1, 1.0],
                emissive: [0.3, 0.1, 0.05],
                cast_shadows: false,
                metallic: 0.0,
                roughness: 1.0,
                material: String::new(),
                visible: true,
                transmission: None,
                ior: None,
            }),
            anim: None,
        });
        r.apply(scene, 0.0);
        for tick in 1..=30 {
            let time = tick as f64 / 60.0;
            r.apply(
                RenderFrame {
                    tick,
                    t_s: time,
                    dt_s: 1.0 / 60.0,
                    ..Default::default()
                },
                time,
            );
            r.capture_rgba(time);
        }
        if enabled {
            let stats = r.water_spray_stats().unwrap();
            assert!(
                stats[0].1.active_upper_bound > 100,
                "occlusion control has no spray"
            );
        }
        images.push(r.capture_rgba(0.5).2);
        if enabled {
            let head = r.water_spray_stats().unwrap()[0].1.ring_head;
            for aa in [Antialiasing::Off, Antialiasing::MsaaTaa, Antialiasing::Msaa] {
                r.set_antialiasing(aa);
                r.capture_rgba(0.5);
                assert_eq!(r.water_spray_stats().unwrap()[0].1.ring_head, head);
            }
        }
    }
    assert!(
        images[0] == images[1],
        "spray rendered through an opaque foreground wall"
    );
}

#[test]
fn tiled_inputs_match_rows_and_a_far_source_remains_available_when_the_camera_returns() {
    for backend in BackendChoice::under_test() {
        let g = gpu(backend, "spray layouts");
        let row = fall(0, false);
        let mut tiled = vec![[0.0; 4]; 512];
        for z in 0..16 {
            for x in 0..32 {
                tiled[(x / 16) * 256 + x % 16 + z * 16] = row[x + z * 32];
            }
        }
        let mut a = Harness::new(&g, &row);
        let mut b = Harness::with_layout(&g, &tiled, true);
        a.step(&g, 1.0 / 60.0, true, true);
        b.step(&g, 1.0 / 60.0, true, true);
        let live = |words: Vec<u32>| {
            let mut out = words
                .as_chunks::<12>()
                .0
                .iter()
                .filter(|p| f32::from_bits(p[7]) > 0.0)
                .map(|p| p.to_vec())
                .collect::<Vec<_>>();
            out.sort();
            out
        };
        assert!(
            live(read(&g, &a.pool.particles)) == live(read(&g, &b.pool.particles)),
            "row/tile source particles differ"
        );
        let mut far = Harness::new(&g, &row);
        g.queue.write_buffer(
            &far.view,
            256,
            bytemuck::cast_slice(&[1000.0f32, 0.0, 1000.0, 0.0]),
        );
        far.step(&g, 1.0 / 60.0, true, true);
        let state = read(&g, &far.pool.state);
        assert!(state[3] > 0);
        assert_eq!(state[1], 0);
        g.queue
            .write_buffer(&far.view, 256, bytemuck::cast_slice(&[0.0f32; 4]));
        far.step(&g, 1.0 / 60.0, false, false);
        assert!(
            read(&g, &far.pool.state)[1] > 0,
            "camera return lost a cached source"
        );
    }
}

/// The particles of one kind alive in the pool.
fn alive(g: &Gpu, h: &Harness, kind: u32) -> Vec<[f32; 8]> {
    read(g, &h.pool.particles)
        .chunks(12)
        .filter(|d| {
            d[8] == kind
                && f32::from_bits(d[7]) > 0.0
                && f32::from_bits(d[3]) < f32::from_bits(d[7])
        })
        .map(|d| std::array::from_fn(|i| f32::from_bits(d[i])))
        .collect()
}

/// A point of `body` whose layer (0.3 m high, 0.018 m^3: a waterplane of 0.06 m^2) went from
/// centre `was` to `now` at `vy` m/s, at (x, z).
fn going(body: u64, x: f32, z: f32, was: f32, now: f32, vy: f32) -> Entering {
    Entering {
        was: [x, was, z],
        now: [x, now, z],
        size: 0.3,
        volume: 0.018,
        velocity: [0.0, vy, 0.0],
        body,
    }
}

#[test]
fn a_body_going_in_throws_one_lattice_crown_and_none_otherwise() {
    for backend in BackendChoice::under_test() {
        let g = gpu(backend, "spray crowns");
        let dt = 1.0 / 60.0;
        // Water 1.5 m deep, its surface at 1 m. A layer going in at 6 m/s: its bottom from 5 cm
        // over the surface to 5 cm under it in a tick.
        let mut h = Harness::new(&g, &flat(1.5));
        let (x, z) = (5.3, 4.2);
        h.entering = vec![going(7, x, z, 1.2, 1.1, -6.0)];
        h.step(&g, dt, true, false);
        let crowns = alive(&g, &h, 6);
        // One crown: three rows about rings 1.1 to 1.5 times its waterplane's radius
        // (sqrt(0.06 / pi), 0.138 m), 23 places each (under 5 cm apart about the middle ring),
        // born over the tick in 3 rows apiece (each 4 cm on at 0.85 x 6 m/s).
        let r = (0.06f32 / std::f32::consts::PI).sqrt();
        assert_eq!(crowns.len(), 207, "{} crown parcels", crowns.len());
        let words = read(&g, &h.pool.particles);
        let parcels: Vec<&[u32]> = words
            .chunks(12)
            .filter(|d| d[8] == 6 && f32::from_bits(d[7]) > 0.0)
            .collect();
        assert!(parcels.iter().all(|d| d[10] == parcels[0][10]), "one key");
        for row in 0..3u32 {
            assert_eq!(parcels.iter().filter(|d| d[11] & 3 == row).count(), 69);
        }
        for d in &crowns {
            let (along, up) = (((d[0] - x).powi(2) + (d[2] - z).powi(2)).sqrt(), d[5]);
            // Born about the ring, at the surface, within the tick's flight of it.
            assert!(along > 1.0 * r && along < 1.5 * r + 0.06, "{d:?}");
            assert!(d[1] > 1.0 && d[1] < 1.02 + 0.1, "{d:?}");
            // Upward, and no faster than falls back within its life.
            let speed = (d[4] * d[4] + d[5] * d[5] + d[6] * d[6]).sqrt();
            assert!(up > 0.0 && speed <= 7.36 * 1.05, "{d:?}");
        }
        // Nothing went in at the next advance: nothing new.
        h.step(&g, dt, true, false);
        assert!(alive(&g, &h, 6).len() <= 207);
        // A layer going in at 1 m/s, one passing over the water, one already under it: none.
        for e in [
            going(7, x, z, 1.2, 1.18, -1.0),
            going(7, x, z, 1.5, 1.4, -6.0),
            going(7, x, z, 0.6, 0.5, -6.0),
        ] {
            let mut h = Harness::new(&g, &flat(1.5));
            h.entering = vec![e];
            h.step(&g, dt, true, false);
            assert!(alive(&g, &h, 6).is_empty(), "{e:?}");
        }
        // The same entries throw the same crowns.
        let mut twin = Harness::new(&g, &flat(1.5));
        let mut first = Harness::new(&g, &flat(1.5));
        for h in [&mut twin, &mut first] {
            h.entering = vec![going(7, x, z, 1.2, 1.1, -6.0)];
            h.step(&g, dt, true, false);
        }
        assert_eq!(
            read(&g, &twin.pool.particles),
            read(&g, &first.pool.particles)
        );
        // Sixteen bodies going in at once take at most 2,048 births between them.
        let mut many = Harness::new(&g, &flat(1.5));
        many.entering = (0..16u64)
            .flat_map(|b| {
                (0..4).map(move |n| {
                    let mut e = going(
                        b,
                        1.0 + (b % 8) as f32 * 1.8,
                        1.5 + (b / 8) as f32 * 4.0,
                        1.2,
                        1.1,
                        -6.0,
                    );
                    e.now[0] += 0.4 * n as f32;
                    e.was[0] = e.now[0];
                    e.volume = 0.2;
                    e
                })
            })
            .collect();
        many.step(&g, dt, true, false);
        assert_eq!(alive(&g, &many, 6).len(), 2048);
    }
}

/// The frame's tally of what the water sounds like from the camera (water_acoustics.rs): per kind
/// its events a second and the power they dissipate over the square of their distance (W/m^2) at
/// the left and the right ear.
fn heard(g: &Gpu, h: &Harness, dt: f32) -> [[f32; 3]; 6] {
    let words = read(g, &h.pipes.acoustics);
    std::array::from_fn(|k| std::array::from_fn(|j| words[3 * k + j] as f32 / (256.0 * dt)))
}

/// Puts the camera at `eye`, `right` its right (the view matrix's first row).
fn look(g: &Gpu, h: &Harness, eye: [f32; 3], right: [f32; 3]) {
    for (c, v) in right.iter().enumerate() {
        g.queue
            .write_buffer(&h.view, 64 + 16 * c as u64, bytemuck::cast_slice(&[*v]));
    }
    g.queue.write_buffer(
        &h.view,
        256,
        bytemuck::cast_slice(&[eye[0], eye[1], eye[2], 0.0]),
    );
}

#[test]
fn a_fall_is_heard_by_the_power_it_dissipates_from_where_it_lies() {
    for backend in BackendChoice::under_test() {
        let g = gpu(backend, "spray acoustics");
        let dt = 1.0 / 60.0;
        let silent = [[0.0; 3]; 6];
        let mut still = Harness::new(&g, &flat(1.0));
        still.listen = true;
        still.step(&g, dt, true, true);
        assert_eq!(heard(&g, &still, dt), silent, "still water made a sound");
        // The fall: 16 faces at x = 8 m, each passing 1 m/s x 0.4 m x 0.5 m = 0.2 m^3/s of water
        // down 6 m, in drops of 1.95 litres (water_spray.rs `rates`).
        let mut h = Harness::new(&g, &fall(0, false));
        h.step(&g, dt, true, true);
        assert_eq!(heard(&g, &h, dt), silent, "heard without listening");
        h.listen = true;
        let watts = 0.2 * 1000.0 * 9.81 * 6.0;
        let expected = |eye: [f32; 3]| -> f32 {
            (0..16)
                .map(|j| {
                    let z = (j as f32 + 0.5) * 0.5 - eye[2];
                    watts / ((8.0 - eye[0]).powi(2) + (6.4 - eye[1]).powi(2) + z * z).max(1.0)
                })
                .sum()
        };
        // Ten frames' tallies, each rounded at random.
        let listen = |h: &mut Harness, eye: [f32; 3], right: [f32; 3]| {
            look(&g, h, eye, right);
            let mut sum = [0.0f32; 3];
            for _ in 0..10 {
                h.step(&g, dt, false, false);
                let falls = heard(&g, h, dt)[0];
                for j in 0..3 {
                    sum[j] += falls[j] / 10.0;
                }
            }
            sum
        };
        let mut powers = vec![];
        for eye in [[38.0, 6.4, 4.0], [68.0, 6.4, 4.0]] {
            let falls = listen(&mut h, eye, [0.0, 0.0, -1.0]);
            assert!(
                (falls[0] / (16.0 * 0.2 / 0.00195) - 1.0).abs() < 0.01,
                "{} events a second",
                falls[0]
            );
            let power = falls[1] + falls[2];
            assert!(
                (power / expected(eye) - 1.0).abs() < 0.03,
                "{power} W/m^2 against {}",
                expected(eye)
            );
            // Straight ahead: as much at the left ear as at the right.
            assert!((falls[1] / falls[2] - 1.0).abs() < 0.05, "{falls:?}");
            powers.push(power);
        }
        // Twice as far, about a quarter of the power.
        let ratio = powers[1] / powers[0];
        let want = expected([68.0, 6.4, 4.0]) / expected([38.0, 6.4, 4.0]);
        assert!((ratio / want - 1.0).abs() < 0.04, "{ratio} against {want}");
        // From beyond its end, looking along it, the fall lies at the right.
        let falls = listen(&mut h, [8.0, 6.4, 40.0], [0.0, 0.0, -1.0]);
        assert!(falls[1] < 0.01 * falls[2], "{falls:?}");
        // Listening leaves the spray as it was (the ring's slots are taken in any order).
        let mut deaf = Harness::new(&g, &fall(0, false));
        let mut hearing = Harness::new(&g, &fall(0, false));
        hearing.listen = true;
        for n in 0..30 {
            deaf.step(&g, dt, n == 0, n == 0);
            hearing.step(&g, dt, n == 0, n == 0);
        }
        let live = |h: &Harness| {
            let mut out: Vec<Vec<u32>> = read(&g, &h.pool.particles)
                .as_chunks::<12>()
                .0
                .iter()
                .filter(|p| f32::from_bits(p[7]) > 0.0)
                .map(|p| p.to_vec())
                .collect();
            out.sort();
            out
        };
        let spray = live(&deaf);
        assert!(
            !spray.is_empty() && spray == live(&hearing),
            "listening changed the spray"
        );
        assert_eq!(read(&g, &deaf.pool.state), read(&g, &hearing.pool.state));
        // The draw's list (water_spray_compute.wgsl `list`): the slots of the parcels alive and
        // no others, in slot order, so the draw blends them in the same order every frame.
        let words = read(&g, &hearing.pool.state);
        let drawn = &words[8..8 + words[5] as usize];
        let alive: Vec<u32> = read(&g, &hearing.pool.particles)
            .as_chunks::<12>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                let (age, life) = (f32::from_bits(p[3]), f32::from_bits(p[7]));
                life > 0.0 && age < life
            })
            .map(|(i, _)| i as u32)
            .collect();
        assert!(!alive.is_empty());
        assert_eq!(
            drawn,
            alive.as_slice(),
            "the list is the parcels alive, in slot order"
        );
    }
}
