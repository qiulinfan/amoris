//! Sub-grid ripples (water.md 57) on their own, without the surface shading: a bobbing point
//! makes a train of rings that runs at the dispersion relation's group speed with its wavelength
//! and dies away; a point drifting with the current makes none; with nothing stirring, the window
//! stays exactly still. Runs on the backend `AMORIS_BACKEND` names (Metal on macOS by default).
use amoris_assets::frame::StirView;
use amoris_render::water_ripples::{Field, Placement, Ripples, SIDE};
use amoris_render::{BackendChoice, Gpu};

const COLUMNS: u32 = 64;
const COLUMN_M: f32 = 0.25;
const DEPTH_M: f32 = 3.0;
/// The middle of the 16 m pool: its window stands over it.
const CENTRE: [f32; 2] = [8.0, 8.0];
const DT: f64 = 1.0 / 60.0;

fn gpu() -> Gpu {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("water ripples require a GPU");
    eprintln!("{:?} on {}", gpu.info.backend, gpu.info.name);
    gpu.device.on_uncaptured_error(std::sync::Arc::new(|error| {
        panic!("water ripples GPU validation failed: {error}");
    }));
    gpu
}

/// A still pool of 64 x 64 columns 0.25 m wide, 3 m deep, flowing at `flow` (m/s).
fn pool(gpu: &Gpu, flow: [f32; 2]) -> wgpu::Buffer {
    let columns: Vec<[f32; 4]> = (0..COLUMNS * COLUMNS)
        .map(|_| [0.0, DEPTH_M, flow[0], flow[1]])
        .collect();
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ripple check columns"),
        size: u64::from(COLUMNS * COLUMNS) * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue
        .write_buffer(&buffer, 0, bytemuck::cast_slice(&columns));
    buffer
}

fn field(columns: &wgpu::Buffer) -> Field<'_> {
    Field {
        origin: [0.0, 0.0],
        cell_m: COLUMN_M,
        size: [COLUMNS, COLUMNS],
        tiled: false,
        columns,
    }
}

/// A duck-sized point (0.006 m^3 over 0.12 m, a waterplane of 0.05 m^2) at `at`.
fn duck(at: [f32; 3]) -> StirView {
    StirView {
        id: 1,
        point: 0,
        at,
        size: 0.12,
        volume: 0.006,
    }
}

/// Steps to `t`, the stirring points at `t` given by `stirs`.
fn step(gpu: &Gpu, ripples: &mut Ripples, columns: &wgpu::Buffer, t: f64, stirs: &[StirView]) {
    ripples.set_stirs(t, stirs);
    let side = COLUMNS as f32 * COLUMN_M;
    let place = Placement::over([0.0; 2], [side; 2], CENTRE);
    assert_eq!(place.centre, CENTRE);
    ripples.prepare(t, place, COLUMN_M);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    ripples.encode(&gpu.device, &gpu.queue, &mut encoder, &field(columns), None);
    gpu.queue.submit([encoder.finish()]);
}

/// The window's rise along with each cell's distance from the pool's centre, metres.
fn rings(gpu: &Gpu, ripples: &Ripples) -> Vec<(f32, f32)> {
    let heights = ripples.heights(&gpu.device, &gpu.queue).unwrap();
    let (origin, cell) = (heights.origin, heights.cell_m);
    let side = SIDE as i32;
    heights
        .rise
        .iter()
        .enumerate()
        .map(|(n, &h)| {
            let c = [n as i32 % side + origin[0], n as i32 / side + origin[1]];
            let x = (c[0] as f32 + 0.5) * cell - CENTRE[0];
            let z = (c[1] as f32 + 0.5) * cell - CENTRE[1];
            ((x * x + z * z).sqrt(), h)
        })
        .collect()
}

/// The ring energy's mean radius (the rise squared, weighted by area) and its total, over the
/// cells farther than `from` metres from the centre.
fn energy(rings: &[(f32, f32)], from: f32) -> (f64, f64) {
    let (mut moment, mut total) = (0.0f64, 0.0f64);
    for &(r, h) in rings {
        if r > from && r < 7.0 {
            let e = f64::from(h) * f64::from(h);
            moment += f64::from(r) * e;
            total += e;
        }
    }
    (moment / total.max(1e-30), total)
}

/// Deep-water capillary-gravity waves of angular frequency `omega`: their wavenumber and group
/// speed, omega^2 = (g k + sigma/rho k^3) tanh(k h).
fn dispersion(omega: f64) -> (f64, f64) {
    let (g, s, h) = (9.81, 7.28e-5, f64::from(DEPTH_M));
    let w2 = |k: f64| (g * k + s * k * k * k) * (k * h).tanh();
    let mut k = omega * omega / g;
    for _ in 0..50 {
        let dk = 1e-6 * k;
        k -= (w2(k) - omega * omega) / ((w2(k + dk) - w2(k - dk)) / (2.0 * dk));
    }
    let dk = 1e-6 * k;
    let group = (w2(k + dk).sqrt() - w2(k - dk).sqrt()) / (2.0 * dk);
    (k, group)
}

#[test]
fn a_bobbing_point_makes_rings_that_run_at_the_group_speed_and_die_away() {
    let gpu = gpu();
    let columns = pool(&gpu, [0.0, 0.0]);
    let mut ripples = Ripples::new(&gpu.device);
    // A wave packet: the point bobs 1 cm at 2 Hz under a Gaussian envelope peaking at 3 s, a
    // second wide, so its rings hold a narrow band of wavelengths around the 2 Hz one.
    let f = 2.0;
    let omega = std::f64::consts::TAU * f;
    let (k, group) = dispersion(omega);
    let peak = 3.0;
    let mut samples = Vec::new();
    for n in 0..=(11.0 / DT) as usize {
        let t = n as f64 * DT;
        let envelope = (-0.5 * ((t - peak) / 1.0).powi(2)).exp();
        let y = -0.02 + 0.01 * envelope * (omega * t).sin();
        step(
            &gpu,
            &mut ripples,
            &columns,
            t,
            &[duck([CENTRE[0], y as f32, CENTRE[1]])],
        );
        if n == (7.0 / DT) as usize || n == (11.0 / DT) as usize {
            samples.push((t, rings(&gpu, &ripples)));
        }
    }
    let (t0, early) = (&samples[0].0, &samples[0].1);
    let (t1, late) = (&samples[1].0, &samples[1].1);
    let (r0, e0) = energy(early, 0.3);
    let (r1, e1) = energy(late, 0.3);
    let speed = (r1 - r0) / (t1 - t0);
    eprintln!(
        "k {k:.2} rad/m (wavelength {:.3} m), group speed {group:.4} m/s; rings at {r0:.3} and \
         {r1:.3} m: {speed:.4} m/s; energy {e0:.3e} then {e1:.3e}",
        std::f64::consts::TAU / k
    );
    assert!(
        (speed / group - 1.0).abs() < 0.1,
        "the rings run at {speed} m/s, the group speed is {group}"
    );
    // Away from the point the packet's energy falls only by damping: the film's k sqrt(nu
    // omega / 8) and viscosity's 2 nu k^2 on the amplitude, twice that on the energy.
    let rate = 2.0e-6 * k * k + k * (1.0e-6 * omega / 8.0).sqrt();
    let kept = (-2.0 * rate * (t1 - t0)).exp();
    eprintln!("energy kept {:.4}, damping alone {kept:.4}", e1 / e0);
    assert!(e1 < e0, "the rings die away: {e0} then {e1}");
    assert!(
        (e1 / e0 / kept - 1.0).abs() < 0.03,
        "the rings keep {} of their energy, damping alone {kept}",
        e1 / e0
    );
    // The crests' spacing along a ray through the packet is the 2 Hz wavelength.
    let side = SIDE as usize;
    let cell = f64::from(ripples.cell_m());
    let centre = side / 2;
    let ray: Vec<f32> = (0..side / 2 - 40)
        .map(|i| late[centre * side + centre + i].1)
        .collect();
    let crossings: Vec<f64> = ray
        .windows(2)
        .enumerate()
        .filter(|(i, w)| {
            let r = (*i as f64 + 0.5) * cell;
            (r - r1).abs() < 1.2 && w[0] * w[1] < 0.0
        })
        .map(|(i, w)| (i as f64 + f64::from(w[0] / (w[0] - w[1]))) * cell)
        .collect();
    assert!(crossings.len() >= 6, "too few crests: {crossings:?}");
    let spacing = (crossings[crossings.len() - 1] - crossings[0]) / (crossings.len() - 1) as f64;
    let wavelength = 2.0 * spacing;
    eprintln!("wavelength along a ray {wavelength:.4} m");
    assert!(
        (wavelength * k / std::f64::consts::TAU - 1.0).abs() < 0.06,
        "the rings' wavelength {wavelength} m against {}",
        std::f64::consts::TAU / k
    );
}

#[test]
fn a_point_drifting_with_the_current_makes_no_ripples_and_a_still_one_against_it_does() {
    let gpu = gpu();
    let flow = [0.4, 0.0];
    let columns = pool(&gpu, flow);
    let mut drifting = Ripples::new(&gpu.device);
    let mut held = Ripples::new(&gpu.device);
    for n in 0..=120 {
        let t = n as f64 * DT;
        let x = 6.0 + flow[0] * t as f32;
        step(&gpu, &mut drifting, &columns, t, &[duck([x, -0.02, 7.0])]);
        step(&gpu, &mut held, &columns, t, &[duck([7.0, -0.02, 7.0])]);
    }
    let (_, moving) = energy(&rings(&gpu, &drifting), 0.0);
    let (_, still) = energy(&rings(&gpu, &held), 0.0);
    eprintln!("ripple energy drifting {moving:.3e}, held against the current {still:.3e}");
    assert!(still > 0.0, "a point held in a current makes waves");
    assert!(
        moving < 1e-3 * still,
        "a point drifting with the water makes none: {moving} against {still}"
    );
}

#[test]
fn with_nothing_stirring_the_window_stays_exactly_still() {
    let gpu = gpu();
    let still = pool(&gpu, [0.0, 0.0]);
    let mut ripples = Ripples::new(&gpu.device);
    // A point floating still on still water, one out of the water, and one past the field.
    let points = [
        duck([4.0, -0.02, 4.0]),
        StirView {
            point: 1,
            ..duck([5.0, 2.0, 5.0])
        },
        StirView {
            point: 2,
            ..duck([30.0, -0.02, 5.0])
        },
    ];
    for n in 0..=30 {
        step(&gpu, &mut ripples, &still, n as f64 * DT, &points);
    }
    let heights = ripples.heights(&gpu.device, &gpu.queue).unwrap();
    assert!(
        heights.rise.iter().all(|&h| h == 0.0),
        "a still point stirred the window"
    );
    // No points at all: no step runs, and the window has no ripples for the shading.
    let flowing = pool(&gpu, [0.3, -0.2]);
    let mut empty = Ripples::new(&gpu.device);
    for n in 0..=30 {
        step(&gpu, &mut empty, &flowing, n as f64 * DT, &[]);
    }
    assert_eq!(empty.window(), [0.0; 4]);
}

mod drawn {
    //! The ripples as the surface shading draws them: with nothing stirring, or with points that
    //! hold still, the frames are exactly those drawn without the layer; a bobbing point shows.
    use super::*;
    use amoris_assets::frame::{
        EnvironmentView, LightKindView, LightView, RenderFrame, WaterLookView, WaterTileView,
        WaterUpdate,
    };
    use amoris_render::{Antialiasing, CameraState, Gtao, Renderer};
    use glam::Vec3;

    const W: u32 = 320;
    const H: u32 = 180;

    /// A pool of 32 x 32 columns of 0.5 m, 2 m deep and still, seen toward the sun so its glints
    /// and the sky's reflection show the surface's slopes.
    fn pool() -> WaterUpdate {
        WaterUpdate {
            id: 7,
            origin: [0.0, 0.0],
            cell_m: 0.5,
            size: [32, 32],
            look: Some(WaterLookView {
                fog: [0.02, 0.1, 0.45, 0.05],
                absorb: [0.3, 0.08, 0.05],
                scatter: [0.01, 0.02, 0.03],
                blocky: false,
                per: 1,
                surround_m: 0.0,
                sea: [false; 4],
            }),
            tiles: [[0, 0], [1, 0], [0, 1], [1, 1]]
                .map(|key| WaterTileView {
                    key,
                    columns: vec![[0.0, 2.0, 0.0, 0.0]; 256],
                })
                .to_vec(),
        }
    }

    fn frame(tick: u64, stirs: Option<Vec<StirView>>) -> RenderFrame {
        let mut f = RenderFrame {
            tick,
            t_s: tick as f64 * DT,
            dt_s: DT,
            reset: tick == 1,
            stirs,
            ..RenderFrame::default()
        };
        if f.reset {
            f.water = vec![pool()];
            f.lights = Some(vec![LightView {
                id: 0,
                kind: LightKindView::Directional,
                position: [0.0; 3],
                direction: Vec3::new(-0.3, -1.0, -0.6).normalize().to_array(),
                color: [1.0, 0.96, 0.9],
                intensity: 6.0,
                range: 0.0,
                inner_deg: 0.0,
                outer_deg: 0.0,
                shadows: true,
            }]);
            f.cameras = Some(vec![]);
            f.environment = Some(EnvironmentView {
                sky: 0,
                sky_color: [0.3, 0.5, 0.8],
                ambient: 1.0,
                baked_gi: String::new(),
                neural_gi: String::new(),
                gi_intensity: 1.0,
                fog_density: 0.0,
                fog_color: [0.6, 0.7, 0.8],
                exposure_ev: 0.0,
                bloom: 0.0,
            });
        }
        f
    }

    fn renderer(gpu: &Gpu, ripples: bool) -> Renderer {
        let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
        r.set_antialiasing(Antialiasing::Msaa);
        r.set_gtao(Gtao::Off);
        r.set_water_foam(false);
        r.set_splashes(false);
        r.set_water_ripples(ripples);
        r.set_camera_override(Some(CameraState::look_at(
            Vec3::new(8.0, 2.5, 4.0),
            Vec3::new(8.0, 0.0, 8.5),
        )));
        r
    }

    /// Plays ticks 1 to `ticks` into both renderers, the stirring points of each tick from
    /// `stirs`, and returns their last frames.
    fn play(
        on: &mut Renderer,
        off: &mut Renderer,
        ticks: u64,
        stirs: impl Fn(f64) -> Option<Vec<StirView>>,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut last = (Vec::new(), Vec::new());
        for tick in 1..=ticks {
            let t = tick as f64 * DT;
            let f = frame(tick, stirs(t));
            on.apply(f.clone(), t);
            off.apply(f, t);
            last = (on.capture_rgba(t).2, off.capture_rgba(t).2);
        }
        last
    }

    #[test]
    fn no_stirring_point_and_still_ones_change_no_pixel_and_a_bobbing_one_does() {
        let gpu = gpu();
        // No points at all.
        let (a, b) = play(
            &mut renderer(&gpu, true),
            &mut renderer(&gpu, false),
            30,
            |_| None,
        );
        assert!(a == b, "the layer changed a frame with nothing stirring");
        // Points that hold still: one floating, one in the air over the water.
        let still = |_| {
            Some(vec![
                duck([8.0, -0.02, 8.0]),
                StirView {
                    point: 1,
                    ..duck([6.0, 1.0, 9.0])
                },
            ])
        };
        let (mut on, mut off) = (renderer(&gpu, true), renderer(&gpu, false));
        let (a, b) = play(&mut on, &mut off, 60, still);
        assert!(on.water_ripples().is_some(), "the points made the layer");
        assert!(a == b, "still points changed a frame");
        // A point bobbing 1 cm at 2 Hz makes rings the shading shows.
        let bobbing = |t: f64| {
            let y = -0.02 + 0.01 * (std::f64::consts::TAU * 2.0 * t).sin();
            Some(vec![duck([8.0, y as f32, 8.0])])
        };
        let (mut on, mut off) = (renderer(&gpu, true), renderer(&gpu, false));
        let (a, b) = play(&mut on, &mut off, 90, bobbing);
        let changed = a
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.as_chunks::<4>().0)
            .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 2))
            .count();
        if let Some(dir) = std::env::var_os("WATER_DUMP") {
            for (name, img) in [("ripples_on", &a), ("ripples_off", &b)] {
                let path = std::path::PathBuf::from(&dir).join(format!("{name}.png"));
                image::save_buffer(path, img, W, H, image::ColorType::Rgba8).unwrap();
            }
        }
        let heights = on.water_ripples().unwrap().unwrap();
        let highest = heights.rise.iter().fold(0.0f32, |m, h| m.max(h.abs()));
        eprintln!("{changed} pixels show the rings (highest {highest:.4} m)");
        assert!(highest > 1e-4, "the bobbing point made no rings");
        assert!(changed > 200, "the rings do not show: {changed} pixels");
    }
}

/// The GPU time of a step (its pass's timestamps) over a deep pool, which runs one depth level,
/// and over one with a shallow shelf, which runs all four; 74 stirring points, as many as the pool
/// party's, on a slow current.
#[test]
#[ignore]
fn time_ripple_steps() {
    let gpu = gpu();
    if !gpu.caps.timestamps {
        eprintln!("no GPU timestamps: skipped");
        return;
    }
    for (name, shelf) in [("deep", 3.0f32), ("shelf", 0.05)] {
        let columns: Vec<[f32; 4]> = (0..COLUMNS * COLUMNS)
            .map(|n| {
                [
                    0.0,
                    if n % COLUMNS < 8 { shelf } else { DEPTH_M },
                    0.05,
                    0.0,
                ]
            })
            .collect();
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ripple timing columns"),
            size: u64::from(COLUMNS * COLUMNS) * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue
            .write_buffer(&buffer, 0, bytemuck::cast_slice(&columns));
        let mut ripples = Ripples::new(&gpu.device);
        let mut profiler =
            amoris_render::profiler::GpuProfiler::new(&gpu.device, &gpu.queue, true, &gpu.info);
        let points = |t: f64| -> Vec<StirView> {
            (0..74)
                .map(|n| {
                    let a = f64::from(n) * 0.7;
                    let y = -0.02 + 0.01 * (t * 9.0 + a).sin();
                    StirView {
                        point: n,
                        ..duck([
                            (8.0 + 5.0 * a.cos()) as f32,
                            y as f32,
                            (8.0 + 5.0 * a.sin()) as f32,
                        ])
                    }
                })
                .collect()
        };
        let mut times = Vec::new();
        for n in 0..240 {
            let t = f64::from(n) * DT;
            ripples.set_stirs(t, &points(t));
            ripples.prepare(t, Placement::over([0.0; 2], [16.0; 2], CENTRE), COLUMN_M);
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            ripples.encode(
                &gpu.device,
                &gpu.queue,
                &mut encoder,
                &field(&buffer),
                Some(&mut profiler),
            );
            profiler.resolve(&mut encoder);
            gpu.queue.submit([encoder.finish()]);
            profiler.after_submit();
            profiler.drain(&gpu.device);
            if n >= 40 {
                times.push(profiler.total_ms());
            }
        }
        times.sort_by(f32::total_cmp);
        eprintln!(
            "{name}: median {:.3} ms, p90 {:.3} ms of GPU time a step",
            times[times.len() / 2],
            times[times.len() * 9 / 10]
        );
    }
}
