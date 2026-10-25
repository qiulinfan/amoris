//! Physics beyond the hydrostatic bulk flow on the resident field (water.md 46 to 49): bodies'
//! displaced volume as a pressure head, ballistic falls from a lip, breaking dissipation and the
//! extended point observations (free-surface rise per column and net inflow).
use amoris_assets::water_field::{
    CheckedCheckpoint, FieldBoundary, FieldCheckpoint, FieldPhysics, FieldRecipe, FieldSea,
    FieldSeaWave, FieldStep, PointVolume,
};
use amoris_render::{
    BackendChoice, Gpu,
    water_compute::Advection,
    water_resident::{ResidentAdvance, ResidentWater},
};
use std::sync::Arc;

const DT: f64 = 1.0 / 60.0;

/// A resident run that follows its own compactions.
struct Run {
    water: ResidentWater,
    checkpoint: Arc<CheckedCheckpoint>,
    recipe: FieldRecipe,
}
impl Run {
    fn new(gpu: &Gpu, field: FieldCheckpoint) -> Self {
        let checkpoint = Arc::new(field.checked().unwrap());
        let recipe = FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new());
        Self {
            water: ResidentWater::new(gpu).unwrap(),
            checkpoint,
            recipe,
        }
    }
    fn step(&mut self, step: &FieldStep, points: &[[f32; 2]]) -> ResidentAdvance {
        let out = self
            .water
            .advance(&self.checkpoint, &self.recipe, step, points, false)
            .unwrap();
        self.follow(&out);
        out
    }
    /// One more step that also reads the complete field back.
    fn read(&mut self, step: &FieldStep) -> FieldCheckpoint {
        let out = self
            .water
            .advance(&self.checkpoint, &self.recipe, step, &[], true)
            .unwrap();
        self.follow(&out);
        (**out.checkpoint.as_ref().unwrap()).as_raw().clone()
    }
    fn follow(&mut self, out: &ResidentAdvance) {
        if let Some(c) = &out.checkpoint {
            self.checkpoint = c.clone();
        }
        self.recipe = out.recipe.clone();
    }
}

fn field(size: [u32; 2], column: impl Fn(u32, u32) -> (f32, f32)) -> FieldCheckpoint {
    let mut columns = Vec::new();
    for k in 0..size[1] {
        for i in 0..size[0] {
            let (bed, depth) = column(i, k);
            columns.push([bed.to_bits(), depth.to_bits(), 0, 0]);
        }
    }
    FieldCheckpoint::new(size, 0.0, columns)
}

fn step(cell: f64, substeps: u32, friction: f64) -> FieldStep {
    FieldStep {
        boundaries: [FieldBoundary::Wall; 4],
        sea: None,
        cell_m: cell,
        substep_dt: DT / f64::from(substeps),
        substeps,
        friction,
        calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
        dry_below: 0.0,
        edits: vec![],
        impulses: vec![],
        displacements: vec![],
        physics: FieldPhysics::default(),
    }
}

fn depth(c: &FieldCheckpoint, i: u32, k: u32) -> f64 {
    f64::from(f32::from_bits(c.columns[(i + c.size[0] * k) as usize][1]))
}
fn volume(c: &FieldCheckpoint) -> f64 {
    c.columns
        .iter()
        .map(|w| f64::from(f32::from_bits(w[1])))
        .sum()
}

#[test]
fn displaced_volume_raises_the_free_surface_and_leaves_water_volume_unchanged() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("displacement requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("displacement validation: {e}")));
        // An 8 x 8 m pool of 0.25 m columns, 1 m deep.
        let size = [32, 32];
        let cell = 0.25;
        let start = field(size, |_, _| (0.0, 1.0));
        let before = volume(&start);
        let mut run = Run::new(&gpu, start);
        // A 1 x 1 m body holding 0.04 m^3 below the free surface: 16 points, one per column.
        let mut s = step(cell, 2, 0.5);
        for k in 0..4 {
            for i in 0..4 {
                s.displacements.push(PointVolume {
                    at: [14.5 + f64::from(i), 14.5 + f64::from(k)],
                    volume_m3: 0.0025,
                    radius: 0.0,
                });
            }
        }
        let far = [[1.5, 1.5], [30.5, 1.5], [1.5, 30.5]];
        let body = [[15.5, 15.5]];
        let points: Vec<[f32; 2]> = far.iter().chain(&body).copied().collect();
        let mut last = None;
        for _ in 0..900 {
            last = Some(run.step(&s, &points));
        }
        let out = last.unwrap();
        // Volume is conserved: the head pushes water aside, it does not edit depths.
        let after = run.read(&s);
        assert!(
            (volume(&after) - before).abs() < 1e-3,
            "water volume changed by {}",
            volume(&after) - before
        );
        // The free surface stands level, 0.04 m^3 over 64 m^2 higher than the water alone.
        let level = 1.0 + 0.04 / 64.0;
        for (n, sample) in out.samples.iter().enumerate() {
            assert!(
                (f64::from(sample[0]) - level).abs() < 2.5e-4,
                "point {n}: free surface {} against {level}",
                sample[0]
            );
        }
        // Under the body the water itself is shallower by the 0.04 m head.
        let under = depth(&after, 15, 15);
        eprintln!(
            "displacement: surfaces {:?} against {level}, water under the body {under}",
            out.samples.iter().map(|s| s[0]).collect::<Vec<_>>()
        );
        assert!(
            (under - (level - 0.04)).abs() < 1e-3,
            "water under the body {under}"
        );
        // The query's depth reaches the free surface, so surface minus depth is the bed.
        assert!((f64::from(out.samples[3][0] - out.samples[3][1])).abs() < 1e-5);
    }
}

#[test]
fn a_moving_body_makes_waves_that_replay_exactly_from_their_checkpoint() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("displacement requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("displacement validation: {e}")));
        let size = [64, 32];
        let cell = 0.25;
        let start = field(size, |_, _| (0.0, 1.2));
        let base = start.clone().checked().unwrap();
        let before = volume(&start);
        let mut run = Run::new(&gpu, start);
        let probe = [[40.5f32, 16.5]];
        let mut quiet = f64::INFINITY;
        let mut moved = 0.0f64;
        for tick in 0..300u32 {
            let mut s = step(cell, 2, 0.02);
            // A 0.06 m^3 body crossing the pool at 2 m/s for one second.
            if tick < 60 {
                let x = 8.0 + f64::from(tick) * 2.0 * DT / cell;
                s.displacements.push(PointVolume {
                    at: [x, 16.0],
                    volume_m3: 0.06,
                    radius: 1.5,
                });
            }
            let out = run.step(&s, &probe);
            let surface = f64::from(out.samples[0][0]);
            if tick < 10 {
                quiet = quiet.min((surface - 1.2).abs());
            }
            moved = moved.max((surface - 1.2).abs());
        }
        eprintln!("moving body: quiet {quiet:e} m, wave {moved:.4} m at the far probe");
        assert!(quiet < 1e-4, "the far probe moved before any wave arrived");
        assert!(
            moved > 2e-3,
            "the body made no wave at the far probe: {moved}"
        );
        let checkpoint = run.checkpoint.clone();
        let recipe = run.recipe.clone();
        // A fresh cache reproduces the authenticated digest from the stored program.
        let mut fresh = ResidentWater::new(&gpu).unwrap();
        fresh.observe(&checkpoint, &recipe, &[]).unwrap();
        let after = run.read(&step(cell, 2, 0.02));
        assert!((volume(&after) - before).abs() < 1e-3);
        assert_ne!(after.digest(), base.digest());
    }
}

#[test]
fn falls_leave_the_lip_and_land_where_the_jet_meets_the_basin() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("falls require a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("falls validation: {e}")));
        // A 4 m shelf over a basin, 16 x 64 columns of 0.25 m: the shelf holds water 0.6 m deep,
        // the basin 0.5 m. The shelf's water runs over the lip at z = 16 columns.
        let size = [16, 64];
        let cell = 0.25;
        let start = field(size, |_, k| if k < 16 { (4.0, 0.6) } else { (0.0, 0.5) });
        let before = volume(&start);
        let mut landed = [[0.0; 64]; 2];
        for (mode, fall) in [0.0, 1.0].into_iter().enumerate() {
            let mut run = Run::new(&gpu, start.clone());
            let mut s = step(cell, 4, 0.02);
            s.physics.fall_min_m = fall;
            for _ in 0..45 {
                run.step(&s, &[]);
            }
            let after = run.read(&s);
            assert!(
                (volume(&after) - before).abs() < 2e-3 * before,
                "mode {mode}: volume changed by {}",
                volume(&after) - before
            );
            for k in 16..64 {
                landed[mode][k as usize] =
                    (0..16).map(|i| depth(&after, i, k)).sum::<f64>() / 16.0 - 0.5;
            }
        }
        // Without falls the shelf pours into the basin's first columns; with them the water
        // lands further out and the foot of the cliff, its first five columns (1.25 m), gets much
        // less. (Pouring as momentum carries it, the jet can pass over the very first column.)
        let centroid = |row: &[f64; 64]| {
            let (mut m, mut w) = (0.0, 0.0);
            for (k, v) in row.iter().enumerate().skip(16) {
                let v = v.max(0.0);
                m += v * k as f64;
                w += v;
            }
            m / w.max(1e-9)
        };
        let (pour, fall) = (centroid(&landed[0]), centroid(&landed[1]));
        let foot = |row: &[f64; 64]| row[16..21].iter().map(|v| v.max(0.0)).sum::<f64>();
        eprintln!(
            "falls: landed centroid {fall:.2} columns against {pour:.2} pouring; foot {:.4} against {:.4} m",
            foot(&landed[1]),
            foot(&landed[0])
        );
        assert!(
            fall > pour + 2.0,
            "falls should land further from the cliff: {fall} against {pour}"
        );
        assert!(
            foot(&landed[1]) < 0.5 * foot(&landed[0]),
            "the cliff's foot should receive much less under falls: {} against {}",
            foot(&landed[1]),
            foot(&landed[0])
        );
    }
}

#[test]
fn breaking_dissipates_a_bore_without_losing_water() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("breaking requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("breaking validation: {e}")));
        let size = [128, 16];
        let cell = 0.25;
        let start = field(size, |i, _| (0.0, if i < 32 { 2.0 } else { 0.5 }));
        let before = volume(&start);
        let energy = |c: &FieldCheckpoint| {
            let mut e = 0.0;
            for (n, w) in c.columns.iter().enumerate() {
                let h = f64::from(f32::from_bits(w[1]));
                let u = f64::from(f32::from_bits(w[2]));
                let v = f64::from(f32::from_bits(w[3]));
                let i = n as u32 % c.size[0];
                // Potential energy of the column and the kinetic energy of its owned faces.
                e += 0.5 * 9.81 * h * h;
                if i > 0 {
                    e += 0.5 * h * (u * u + v * v);
                }
            }
            e
        };
        let mut energies = [0.0; 2];
        for (mode, breaking) in [false, true].into_iter().enumerate() {
            let mut run = Run::new(&gpu, start.clone());
            let mut s = step(cell, 4, 0.0);
            s.physics.breaking = breaking;
            for _ in 0..180 {
                run.step(&s, &[]);
            }
            let after = run.read(&s);
            assert!((volume(&after) - before).abs() < 1e-3 * before);
            assert!(after.validate().is_ok());
            energies[mode] = energy(&after);
        }
        eprintln!("bore energy without and with breaking: {energies:?}");
        assert!(
            energies[1] < energies[0],
            "breaking should remove energy: {energies:?}"
        );
    }
}

#[test]
fn queries_report_the_surface_rise_per_column_and_the_net_inflow() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("queries require a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("query validation: {e}")));
        // A free surface tilted along x by 0.01 per column, and one face carrying 1 m/s from
        // column 7 into column 8.
        let size = [16, 16];
        let mut cp = field(size, |i, _| (0.0, 1.0 + 0.01 * i as f32));
        for k in 0..16 {
            cp.columns[(8 + 16 * k) as usize][2] = 1.0f32.to_bits();
        }
        let cp = Arc::new(cp.checked().unwrap());
        let recipe = FieldRecipe::new(cp.key(), vec![], cp.digest(), String::new());
        let mut water = ResidentWater::new(&gpu).unwrap();
        let out = water
            .observe(&cp, &recipe, &[[4.5, 4.5], [7.5, 4.5], [8.5, 4.5]])
            .unwrap();
        assert!(
            (out.samples[0][4] - 0.01).abs() < 1e-5,
            "{:?}",
            out.samples[0]
        );
        assert_eq!(out.samples[0][5], 0.0);
        assert_eq!(out.samples[0][6], 0.0);
        // Column 7 loses, column 8 gains, the face's discharge at its centred depth.
        let q = 0.5 * (1.07 + 1.08);
        assert!((out.samples[1][6] + q).abs() < 1e-4, "{:?}", out.samples[1]);
        assert!((out.samples[2][6] - q).abs() < 1e-4, "{:?}", out.samples[2]);
    }
}

/// The standing wave's period at the left wall, from upward zero crossings of the surface.
fn standing_period(
    gpu: &Gpu,
    depth: f32,
    wavelength: f32,
    cell: f64,
    physics: FieldPhysics,
    seconds: f64,
) -> (f64, f64) {
    let size = [128, 16];
    let k = std::f32::consts::TAU / wavelength;
    let cp = field(size, |i, _| {
        let x = (i as f32 + 0.5) * cell as f32;
        (0.0, depth + 0.01 * (k * x).cos())
    });
    let before = volume(&cp);
    let mut run = Run::new(gpu, cp);
    let mut s = step(cell, 2, 0.0);
    s.physics = physics;
    let mut crossings = Vec::new();
    let mut last = 0.0;
    let ticks = (seconds / DT) as u32;
    for tick in 0..ticks {
        let out = run.step(&s, &[[0.5, 8.5]]);
        let eta = f64::from(out.samples[0][0]) - f64::from(depth);
        if tick > 0 && last < 0.0 && eta >= 0.0 {
            // Linear interpolation of the crossing time within the tick.
            crossings.push((f64::from(tick) - eta / (eta - last)) * DT);
        }
        last = eta;
    }
    let after = run.read(&s);
    let error = (volume(&after) - before).abs() / before;
    let period = (crossings[crossings.len() - 1] - crossings[0]) / (crossings.len() - 1) as f64;
    (period, error)
}

#[test]
fn surface_waves_run_at_the_airy_speed_for_their_depth() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("dispersion requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("dispersion validation: {e}")));
        let depth = 2.0f32;
        for wavelength in [2.0f32, 4.0, 8.0] {
            let k = std::f64::consts::TAU / f64::from(wavelength);
            let airy = std::f64::consts::TAU / (9.81 * k * (k * f64::from(depth)).tanh()).sqrt();
            let shallow = std::f64::consts::TAU / (k * (9.81 * f64::from(depth)).sqrt());
            let dispersive_physics = FieldPhysics {
                dispersion: true,
                ..FieldPhysics::default()
            };
            let (dispersive, error) =
                standing_period(&gpu, depth, wavelength, 0.25, dispersive_physics, 12.0);
            let (plain, _) =
                standing_period(&gpu, depth, wavelength, 0.25, FieldPhysics::default(), 12.0);
            eprintln!(
                "lambda {wavelength} m at {depth} m: dispersive period {dispersive:.4} s (Airy {airy:.4}), shallow-water {plain:.4} s ({shallow:.4}); volume error {error:e}"
            );
            assert!(error < 1e-5, "volume error {error}");
            assert!(
                (dispersive / airy - 1.0).abs() < 0.03,
                "lambda {wavelength}: period {dispersive} against Airy {airy}"
            );
        }
    }
}

#[test]
fn non_hydrostatic_waves_run_at_their_theory_s_speed_near_airy_s() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("non-hydrostatic water requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("non-hydrostatic validation: {e}")));
        let depth = 2.0f64;
        let physics = FieldPhysics {
            nonhydrostatic: true,
            ..FieldPhysics::default()
        };
        for wavelength in [8.0f32, 16.0] {
            let k = std::f64::consts::TAU / f64::from(wavelength);
            let kh = k * depth;
            let airy = std::f64::consts::TAU / (9.81 * k * kh.tanh()).sqrt();
            // The one-layer depth-integrated model's own relation: w^2 = g k^2 h / (1 + (kh)^2/4).
            let theory =
                std::f64::consts::TAU / (9.81 * k * k * depth / (1.0 + kh * kh / 4.0)).sqrt();
            let shallow = std::f64::consts::TAU / (k * (9.81 * depth).sqrt());
            let (period, error) =
                standing_period(&gpu, depth as f32, wavelength, 0.25, physics, 16.0);
            eprintln!(
                "lambda {wavelength} m at {depth} m: non-hydrostatic period {period:.4} s (its theory {theory:.4}, Airy {airy:.4}, shallow water {shallow:.4}); volume error {error:e}"
            );
            assert!(error < 1e-5, "volume error {error}");
            assert!(
                (period / theory - 1.0).abs() < 0.03,
                "lambda {wavelength}: period {period} against the model's {theory}"
            );
            assert!((period - airy).abs() < (shallow - airy).abs() * 0.5);
        }
    }
}

/// A solitary wave on a flat bed: the depth-integrated non-hydrostatic model keeps its crest and
/// its speed, sqrt(g (h + A)); shallow water alone steepens it into a bore.
fn solitary(gpu: &Gpu, physics: FieldPhysics, seconds: f64) -> (f64, f64) {
    let (h, a, cell) = (1.0f64, 0.2f64, 0.25f64);
    let size = [512u32, 16u32];
    let kappa = (3.0 * a / (4.0 * h * h * h)).sqrt();
    let c = (9.81 * (h + a)).sqrt();
    let x0 = 20.0;
    let eta = |x: f64| a / (kappa * (x - x0)).cosh().powi(2);
    let mut columns = Vec::new();
    for _ in 0..size[1] {
        for i in 0..size[0] {
            let x = (f64::from(i) + 0.5) * cell;
            let face = f64::from(i) * cell;
            let u = c * eta(face) / (h + eta(face));
            columns.push([
                (-h as f32).to_bits(),
                ((h + eta(x)) as f32).to_bits(),
                (u as f32).to_bits(),
                0,
            ]);
        }
    }
    let mut run = Run::new(gpu, FieldCheckpoint::new(size, 0.0, columns));
    let mut s = step(cell, 4, 0.0);
    s.calm_per_s = 0.0;
    s.physics = physics;
    for _ in 0..(seconds / DT) as u32 {
        run.step(&s, &[]);
    }
    let out = run.read(&s);
    let (mut crest, mut at) = (f64::MIN, 0.0);
    for i in 0..size[0] {
        let d = f64::from(f32::from_bits(out.columns[(i + size[0] * 8) as usize][1]));
        if d - h > crest {
            crest = d - h;
            at = (f64::from(i) + 0.5) * cell;
        }
    }
    (crest / a, (at - x0) / (c * seconds))
}

#[test]
fn a_non_hydrostatic_solitary_wave_keeps_its_crest_and_speed() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("non-hydrostatic water requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("solitary validation: {e}")));
        let physics = FieldPhysics {
            nonhydrostatic: true,
            ..FieldPhysics::default()
        };
        let (crest, speed) = solitary(&gpu, physics, 10.0);
        let (plain_crest, plain_speed) = solitary(&gpu, FieldPhysics::default(), 10.0);
        eprintln!(
            "solitary wave after 10 s: non-hydrostatic crest {crest:.3} of its height at {speed:.3} of its speed; shallow water {plain_crest:.3} at {plain_speed:.3}"
        );
        assert!(
            crest > 0.85 && (speed - 1.0).abs() < 0.05,
            "{crest} {speed}"
        );
    }
}

/// The water's energy over its rest state, J per kg/m^3 of density: kinetic on each face between
/// wet columns with its mean depth, potential from the surface's departure from its mean level and,
/// with `vertical`, the non-hydrostatic model's vertical kinetic energy D W^2 / 2 with
/// W = -D/2 div U over a flat bed (water_nh.wgsl), in a basin closed by walls.
fn energy(c: &FieldCheckpoint, cell: f64, vertical: bool) -> f64 {
    let at = |i: u32, k: u32| c.columns[(i + c.size[0] * k) as usize].map(f32::from_bits);
    let (mut kinetic, mut volume, mut area, mut squares) = (0.0, 0.0, 0.0, 0.0);
    for k in 0..c.size[1] {
        for i in 0..c.size[0] {
            let w = at(i, k);
            if w[1] <= 0.0 {
                continue;
            }
            if vertical {
                let east = if i + 1 < c.size[0] {
                    at(i + 1, k)[2]
                } else {
                    0.0
                };
                let north = if k + 1 < c.size[1] {
                    at(i, k + 1)[3]
                } else {
                    0.0
                };
                let west = if i > 0 { w[2] } else { 0.0 };
                let south = if k > 0 { w[3] } else { 0.0 };
                let up = -0.5 * f64::from(w[1]) * f64::from(east - west + north - south) / cell;
                kinetic += 0.5 * f64::from(w[1]) * up * up;
            }
            let surface = f64::from(w[0] + w[1]);
            volume += surface;
            area += 1.0;
            squares += surface * surface;
            if i > 0 && at(i - 1, k)[1] > 0.0 {
                kinetic += 0.25 * f64::from(w[1] + at(i - 1, k)[1]) * f64::from(w[2] * w[2]);
            }
            if k > 0 && at(i, k - 1)[1] > 0.0 {
                kinetic += 0.25 * f64::from(w[1] + at(i, k - 1)[1]) * f64::from(w[3] * w[3]);
            }
        }
    }
    let potential = 0.5 * 9.81 * (squares - volume * volume / area);
    (kinetic + potential) * cell * cell
}

#[test]
fn deep_non_hydrostatic_water_stays_stable_and_gains_no_energy() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("non-hydrostatic water requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("deep basin validation: {e}")));
        // A closed basin 8.8 m deep at half-metre columns, with a hump. The exact pressure all but
        // holds its short waves still; one Chebyshev cycle of 48 iterations, whose error changes
        // sign across the spectrum, reversed gravity on some of them and broke the field apart
        // within 4 s (water_nh.rs `CYCLE`).
        let cell = 0.5;
        let size = [64u32, 64u32];
        let start = field(size, |i, k| {
            let d = ((i as f32 - 32.0).powi(2) + (k as f32 - 32.0).powi(2)) / 16.0;
            (-8.8, 8.8 + 0.3 * (-d).exp())
        });
        let before = energy(&start, cell, true);
        let mut run = Run::new(&gpu, start);
        let mut s = step(cell, 4, 0.0);
        s.calm_per_s = 0.0;
        s.physics.nonhydrostatic = true;
        let (mut fastest, mut most, mut last) = (0.0f32, 0.0f64, 0.0f64);
        for second in 0..20 {
            for _ in 0..59 {
                fastest = fastest.max(run.step(&s, &[]).summary.max_speed);
            }
            let now = run.read(&s);
            let e = energy(&now, cell, true);
            most = most.max(e);
            last = e;
            if second % 5 == 4 {
                eprintln!(
                    "deep basin after {} s: energy {:.4} of the start's ({:.4} without the vertical), fastest {fastest:.3} m/s",
                    second + 1,
                    e / before,
                    energy(&now, cell, false) / before
                );
            }
        }
        assert!(fastest < 0.5, "the basin ran at {fastest} m/s");
        // The forward-backward step keeps a nearby energy, so this one swings by a few percent
        // with the waves; it must not grow.
        assert!(
            most < before * 1.05 && last < before,
            "energy rose to {} and ended at {} of the start's",
            most / before,
            last / before
        );
    }
}

#[test]
fn still_water_beside_a_cliff_stays_still_with_dispersion() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("dispersion requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("cliff validation: {e}")));
        // A basin 5 m deep along a cliff standing 10 m over the water, still: the split must not
        // average the rock into the water beside it and set the water moving.
        let size = [64u32, 32u32];
        let start = field(size, |_, k| if k >= 24 { (10.0, 0.0) } else { (-5.0, 5.0) });
        let mut run = Run::new(&gpu, start);
        let mut s = step(0.5, 2, 0.01);
        s.calm_per_s = 0.0;
        s.physics.dispersion = true;
        for _ in 0..120 {
            run.step(&s, &[]);
        }
        let out = run.read(&s);
        let (mut fastest, mut off) = (0.0f32, 0.0f64);
        for k in 0..24 {
            for i in 0..size[0] {
                let c = out.columns[(i + size[0] * k) as usize].map(f32::from_bits);
                fastest = fastest.max(c[2].abs()).max(c[3].abs());
                off = off.max((f64::from(c[1]) - 5.0).abs());
            }
        }
        assert!(
            fastest < 1e-3 && off < 1e-3,
            "speed {fastest} m/s, depth off by {off} m"
        );
    }
}

#[test]
#[ignore]
fn time_dispersive_steps() {
    let gpu = Gpu::headless(BackendChoice::from_env()).unwrap();
    gpu.device
        .on_uncaptured_error(Arc::new(|e| panic!("timing validation: {e}")));
    for size in [[128u32, 128u32], [256, 256], [512, 256], [512, 512]] {
        // The shallow-water step with each transport of the face velocities (water.md 61.1):
        // this water is deep and smooth, so the regions' hybrid one runs MacCormack's.
        for (dispersion, nonhydrostatic, advection) in [
            (false, false, Advection::MacCormack),
            (false, false, Advection::Conservative),
            (false, false, Advection::Hybrid),
            (true, false, Advection::Hybrid),
            (false, true, Advection::Hybrid),
        ] {
            let cp = field(size, |i, k| {
                (
                    0.0,
                    1.5 + 0.02 * ((i as f32 * 0.3).sin() + (k as f32 * 0.2).cos()),
                )
            });
            let mut run = Run::new(&gpu, cp);
            run.water.set_advection(advection);
            let mut s = step(0.25, 4, 0.02);
            s.physics.dispersion = dispersion;
            s.physics.nonhydrostatic = nonhydrostatic;
            for _ in 0..10 {
                run.step(&s, &[]);
            }
            let start = std::time::Instant::now();
            let n = 60;
            for _ in 0..n {
                run.step(&s, &[]);
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
            eprintln!(
                "{size:?} dispersion {dispersion} non-hydrostatic {nonhydrostatic} {advection:?}: {ms:.3} ms a step of 4 substeps (wall, with a 64-byte readback)"
            );
        }
    }
}

#[test]
fn dispersive_breaking_water_on_a_beach_stays_finite_conserves_and_replays() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("dispersion requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("beach validation: {e}")));
        // 64 x 32 m: a bed rising from 4 m deep to 1 m above the still level, a 0.6 m hump of
        // water offshore that runs up the beach, and a body crossing the shallows.
        let size = [128, 64];
        let cell = 0.5;
        let start = field(size, |i, k| {
            let x = i as f32 * cell as f32;
            let bed = -4.0 + 5.0 * (x / 64.0);
            let d = ((x - 12.0).powi(2) + (k as f32 * 0.5 - 16.0).powi(2)) / 16.0;
            (bed, (0.6 * (-d).exp() - bed).max(0.0))
        });
        let before = volume(&start);
        let mut run = Run::new(&gpu, start);
        let mut s = step(cell, 4, 0.02);
        s.physics.dispersion = true;
        s.physics.breaking = true;
        let mut last = None;
        for tick in 0..600u32 {
            s.displacements = vec![PointVolume {
                at: [40.0 + f64::from(tick % 200) * 0.1, 32.0],
                volume_m3: 0.2,
                radius: 2.0,
            }];
            last = Some(run.step(&s, &[[20.5, 32.5], [100.5, 32.5]]));
        }
        let out = last.unwrap();
        assert!(out.samples.iter().flatten().all(|v| v.is_finite()));
        // A fresh cache replays the stored program across its compaction and must match.
        let mut fresh = ResidentWater::new(&gpu).unwrap();
        fresh
            .observe(&run.checkpoint, &run.recipe, &[[20.5, 32.5]])
            .expect("the dispersive program replays to its digest");
        let after = run.read(&s);
        after.validate().unwrap();
        let error = (volume(&after) - before).abs() / before;
        eprintln!("beach: volume error {error:e} after 10 s");
        assert!(error < 1e-4, "volume error {error}");
    }
}

#[test]
fn a_dispersive_field_takes_swell_from_the_sea_at_intermediate_depth_at_airy_speed() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("dispersion requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("sea swell validation: {e}")));
        // A 256 m channel 6 m deep and a 10 s swell from its open west edge, a twentieth of whose
        // length in this depth is under 4 m: too short a wave for the long-wave limit.
        let (cell, depth, period) = (0.5, 6.0f64, 10.0);
        let mut run = Run::new(&gpu, field([512, 16], |_, _| (-6.0, 6.0)));
        let omega = std::f64::consts::TAU / period;
        let mut s = step(cell, 4, 0.0);
        s.boundaries = [
            FieldBoundary::Sea,
            FieldBoundary::Radiation {
                surface_relative_m: 0.0,
                outward_velocity_mps: 0.0,
            },
            FieldBoundary::Wall,
            FieldBoundary::Wall,
        ];
        s.physics.dispersion = true;
        // A swell travels: no calm damping (water.md 4).
        s.calm_per_s = 0.0;
        // Airy's wavenumber for the depth, by Newton.
        let mut k = omega / (9.81 * depth).sqrt();
        for _ in 0..20 {
            let f = 9.81 * k * (k * depth).tanh() - omega * omega;
            let df = 9.81 * ((k * depth).tanh() + k * depth / (k * depth).cosh().powi(2));
            k -= f / df;
        }
        let airy = omega / k;
        // Gauges every 10 m from 10 to 150 m in.
        let gauges: Vec<[f32; 2]> = (1..=15).map(|n| [n as f32 * 20.0 + 0.5, 8.0]).collect();
        let mut series = vec![Vec::new(); gauges.len()];
        for tick in 0..(60 * 60) {
            let t = f64::from(tick) * DT;
            // The swell rises over two periods, so no broad start reaches the gauges.
            let rise = (t / 20.0).min(1.0);
            s.sea = Some(FieldSea {
                level_relative_m: 0.0,
                waves: vec![FieldSeaWave {
                    amplitude_m: 0.05 * rise * rise * (3.0 - 2.0 * rise),
                    wave_vector_rad_m: [omega * omega / 9.81, 0.0],
                    angular_frequency_rad_s: omega,
                    phase_at_start_rad: (std::f64::consts::PI - omega * t)
                        .rem_euclid(std::f64::consts::TAU)
                        - std::f64::consts::PI,
                }],
            });
            let out = run.step(&s, &gauges);
            for (row, sample) in series.iter_mut().zip(&out.samples) {
                row.push((t + DT, f64::from(sample[0])));
            }
        }
        // Each gauge's amplitude and phase at the swell's frequency, by least squares, after the
        // rise has passed and before the far edge's echo returns.
        let fit = |g: usize| -> (f64, f64) {
            let (mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for &(t, y) in series[g].iter().filter(|(t, _)| *t > 38.0 && *t < 58.0) {
                let (sn, cs) = (omega * t).sin_cos();
                ss += sn * sn;
                sc += sn * cs;
                cc += cs * cs;
                ys += y * sn;
                yc += y * cs;
            }
            let det = ss * cc - sc * sc;
            let (a, b) = ((ys * cc - yc * sc) / det, (yc * ss - ys * sc) / det);
            (a.hypot(b), b.atan2(a))
        };
        let fits: Vec<(f64, f64)> = (0..gauges.len()).map(fit).collect();
        // From 30 to 140 m, past the fade where the swell turns from a long wave to an Airy wave.
        let (first, last) = (fits[2], fits[13]);
        let turns = (first.1 - last.1).rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
        let wavelength = 110.0 / (turns + (110.0 * k / std::f64::consts::TAU).floor());
        let speed = wavelength / period;
        let heights: Vec<f64> = fits[2..].iter().map(|f| f.0).collect();
        let (low, high) = heights
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), &v| (l.min(v), h.max(v)));
        eprintln!(
            "10 s swell at 6 m: {speed:.2} m/s against Airy {airy:.2} (long-wave {:.2}); amplitude {low:.4} to {high:.4} m of 0.05",
            (9.81 * depth).sqrt()
        );
        // Slower than a long wave: the surface waves carry most of it at Airy's speed.
        assert!(
            (speed / airy - 1.0).abs() < 0.035 && speed < 0.995 * (9.81 * depth).sqrt(),
            "{speed} against {airy}"
        );
        // It enters at the sea's height and keeps it: no calm damping, no decay, no ripple.
        assert!(
            low > 0.048 && high < 0.054 && high / low < 1.02,
            "{heights:?}"
        );
    }
}

/// Research harness, not a check: non-hydrostatic water in small basins. `NH_CASE` is `bump`
/// (a flat basin 8.8 m deep with a hump), `step` (the same beside a submarine cliff from 3 to 8.8 m)
/// or `sea` (the flat basin under the surf sample's swell from its west side).
#[test]
#[ignore]
fn nh_research() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("research needs a GPU");
    let case = std::env::var("NH_CASE").unwrap_or_else(|_| "bump".into());
    let size = [128u32, 128u32];
    let deep = 8.8f32;
    let bed = |k: u32| -> f32 {
        if case != "step" {
            return -deep;
        }
        match k {
            0..=15 => -3.0,
            16 => -5.0,
            17 => -7.0,
            _ => -deep,
        }
    };
    let start = field(size, |i, k| {
        let b = bed(k);
        let d = ((i as f32 - 64.0).powi(2) + (k as f32 - 64.0).powi(2)) / 64.0;
        let hump = if case == "sea" { 0.0 } else { 0.5 * (-d).exp() };
        (b, hump - b)
    });
    let mut run = Run::new(&gpu, start);
    let mut s = step(0.5, 4, 0.01);
    s.calm_per_s = 0.0;
    s.physics.nonhydrostatic = std::env::var("NH_OFF").is_err();
    s.physics.breaking = std::env::var("NH_BREAKING").is_ok();
    if case == "sea" {
        s.boundaries[0] = FieldBoundary::Sea;
    }
    let k = std::f64::consts::TAU / 624.0;
    let omega = (9.81 * k).sqrt();
    let ticks: u32 = std::env::var("NH_TICKS").map_or(3600, |v| v.parse().unwrap());
    for tick in 0..ticks {
        let t = f64::from(tick) * DT;
        if case == "sea" {
            s.sea = Some(FieldSea {
                level_relative_m: 0.0,
                waves: vec![FieldSeaWave {
                    amplitude_m: 1.2,
                    wave_vector_rad_m: [k, 0.0],
                    angular_frequency_rad_s: omega,
                    phase_at_start_rad: (std::f64::consts::PI - omega * t)
                        .rem_euclid(std::f64::consts::TAU)
                        - std::f64::consts::PI,
                }],
            });
        }
        let out = run.step(&s, &[]);
        if tick % 60 == 59 || out.summary.max_speed > 20.0 {
            let cp = run.read(&s);
            let (mut kinetic, mut potential) = (0.0f64, 0.0f64);
            for c in &cp.columns {
                let c = c.map(f32::from_bits);
                if c[1] > 0.01 {
                    let surface = f64::from(c[0] + c[1]);
                    let b = f64::from(c[0]);
                    kinetic += 0.5 * f64::from(c[1]) * f64::from(c[2] * c[2] + c[3] * c[3]);
                    potential += 0.5 * 9.81 * (surface * surface - b * b);
                }
            }
            eprintln!(
                "{case} {:5.1} s: max speed {:6.3} m/s, energy kinetic {:10.3} potential {:10.3} (kJ/m density)",
                t + DT,
                out.summary.max_speed,
                kinetic * 0.25,
                potential * 0.25
            );
            if out.summary.max_speed > 20.0 {
                panic!("blew up");
            }
        }
    }
}

/// Research harness, not a check: the surf beach sample's bay and swell without bodies, its
/// largest speed over time. `SURF_DISPERSION`, `SURF_NH` and `SURF_BREAKING` (0 or 1) override the
/// region's own physics, `SURF_SUBSTEPS` its 4 substeps, `SURF_TICKS` sets the length and
/// `SURF_WHERE` prints the fastest columns.
#[test]
#[ignore]
fn surf_bay_research() {
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("surf research needs a GPU");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/surf-beach");
    let scene: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("scene.json")).unwrap()).unwrap();
    let entities = scene["entities"].as_array().unwrap();
    let region = entities
        .iter()
        .find_map(|e| e["components"].get("WaterRegion"))
        .unwrap();
    let sea = entities
        .iter()
        .find_map(|e| e["components"].get("Sea"))
        .unwrap();
    let size: [u32; 2] = serde_json::from_value(region["size"].clone()).unwrap();
    let mut columns = vec![[0u32; 4]; (size[0] * size[1]) as usize];
    for tile in region["tiles"].as_array().unwrap() {
        let key = [
            tile["key"][0].as_u64().unwrap() as u32,
            tile["key"][1].as_u64().unwrap() as u32,
        ];
        for n in 0..256u32 {
            let (i, k) = (key[0] * 16 + n % 16, key[1] * 16 + n / 16);
            let bed = tile["bed"][n as usize].as_f64().unwrap() as f32;
            let depth = tile["depth"][n as usize].as_f64().unwrap() as f32;
            columns[(i + size[0] * k) as usize] = [bed.to_bits(), depth.to_bits(), 0, 0];
        }
    }
    let mut run = Run::new(&gpu, FieldCheckpoint::new(size, 0.0, columns));
    let flag = |name: &str, key: &str| {
        std::env::var(name).map_or(region["physics"][key].as_bool().unwrap_or(false), |v| {
            v != "0"
        })
    };
    let substeps = std::env::var("SURF_SUBSTEPS").map_or(4, |v| v.parse().unwrap());
    let mut s = step(0.5, substeps, 0.01);
    s.calm_per_s = 0.0;
    s.boundaries = [
        FieldBoundary::Sea,
        FieldBoundary::Wall,
        FieldBoundary::Wall,
        FieldBoundary::Wall,
    ];
    s.physics.dispersion = flag("SURF_DISPERSION", "dispersion");
    s.physics.breaking = flag("SURF_BREAKING", "breaking");
    s.physics.nonhydrostatic = flag("SURF_NH", "nonhydrostatic");
    let ticks: u32 = std::env::var("SURF_TICKS").map_or(5400, |v| v.parse().unwrap());
    let waves: Vec<(f64, [f64; 2], f64, f64)> = sea["waves"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            let k = std::f64::consts::TAU / w["length"].as_f64().unwrap();
            let bearing = w["toward_deg"].as_f64().unwrap().to_radians();
            (
                0.5 * w["height"].as_f64().unwrap()
                    * std::env::var("SURF_SCALE").map_or(1.0, |v| v.parse::<f64>().unwrap()),
                [k * bearing.sin(), -k * bearing.cos()],
                (9.81 * k).sqrt(),
                w["phase"].as_f64().unwrap(),
            )
        })
        .collect();
    for tick in 0..ticks {
        let t = f64::from(tick) * DT;
        s.sea = Some(FieldSea {
            level_relative_m: 0.0,
            waves: waves
                .iter()
                .map(|&(a, k, omega, phase)| FieldSeaWave {
                    amplitude_m: a,
                    wave_vector_rad_m: k,
                    angular_frequency_rad_s: omega,
                    phase_at_start_rad: (std::f64::consts::PI + phase - omega * t)
                        .rem_euclid(std::f64::consts::TAU)
                        - std::f64::consts::PI,
                })
                .collect(),
        });
        let every = std::env::var("SURF_EVERY").map_or(300, |v| v.parse().unwrap());
        if tick % every == every - 1 && std::env::var_os("SURF_WHERE").is_some() {
            // Where the fastest faces are: column, depth, bed and both faces' speeds.
            let cp = run.read(&s);
            let mut worst: Vec<(f32, u32, u32, f32, f32)> = Vec::new();
            for k in 0..size[1] {
                for i in 0..size[0] {
                    let c = cp.columns[(i + size[0] * k) as usize];
                    let speed = f32::from_bits(c[2]).abs().max(f32::from_bits(c[3]).abs());
                    worst.push((speed, i, k, f32::from_bits(c[1]), f32::from_bits(c[0])));
                }
            }
            worst.sort_by(|a, b| b.0.total_cmp(&a.0));
            // The strongest vortex (vorticity at a corner, 1/s) and the water's energy over still
            // water at level 0: kinetic from the faces, potential from the surface.
            let at = |i: u32, k: u32| cp.columns[(i + size[0] * k) as usize].map(f32::from_bits);
            let mut spin = (0.0f32, 0, 0);
            let (mut kinetic, mut potential) = (0.0f64, 0.0f64);
            for k in 1..size[1] {
                for i in 1..size[0] {
                    let (c, l, d) = (at(i, k), at(i - 1, k), at(i, k - 1));
                    if c[1] > 0.01 && l[1] > 0.01 && d[1] > 0.01 {
                        let w = ((c[3] - l[3]) - (c[2] - d[2])) / 0.5;
                        if w.abs() > spin.0.abs() {
                            spin = (w, i, k);
                        }
                    }
                    if c[1] > 0.01 {
                        let s = f64::from(c[0] + c[1]);
                        let b = f64::from(c[0]).max(0.0);
                        kinetic += 0.5
                            * 1000.0
                            * f64::from(c[1])
                            * f64::from(c[2] * c[2] + c[3] * c[3])
                            * 0.25;
                        potential += 0.5 * 1000.0 * 9.81 * (s * s - b * b) * 0.25;
                    }
                }
            }
            eprintln!(
                "{:>5.1} s vortex {:.2} /s at ({:.1}, {:.1}); energy kinetic {:.1} MJ, potential {:.1} MJ",
                f64::from(tick + 1) * DT,
                spin.0,
                f64::from(spin.1) * 0.5,
                f64::from(spin.2) * 0.5,
                kinetic / 1e6,
                potential / 1e6
            );
            eprintln!(
                "{:>5.1} s fastest: {:?}",
                f64::from(tick + 1) * DT,
                worst[..5]
                    .iter()
                    .map(|w| format!(
                        "{:.1} m/s at ({:.1}, {:.1}) depth {:.2} bed {:.2}",
                        w.0,
                        f64::from(w.1) * 0.5 + 0.25,
                        f64::from(w.2) * 0.5 + 0.25,
                        w.3,
                        w.4
                    ))
                    .collect::<Vec<_>>()
            );
        }
        let out = run.step(&s, &[]);
        // `SURF_DUMP=speed`: once the field runs faster, the surface and faces around its fastest
        // column every few ticks.
        let dump: f32 = std::env::var("SURF_DUMP").map_or(f32::INFINITY, |v| v.parse().unwrap());
        if out.summary.max_speed > dump && tick % 3 == 0 {
            let cp = run.read(&s);
            let at = |i: u32, k: u32| cp.columns[(i + size[0] * k) as usize].map(f32::from_bits);
            let (mut best, mut fi, mut fk) = (0.0f32, 0, 0);
            for k in 0..size[1] {
                for i in 0..size[0] {
                    let c = at(i, k);
                    if c[2].abs().max(c[3].abs()) > best {
                        (best, fi, fk) = (c[2].abs().max(c[3].abs()), i, k);
                    }
                }
            }
            eprintln!(
                "{:.2} s fastest {best:.1} m/s at column ({fi}, {fk})",
                f64::from(tick + 1) * DT
            );
            for (name, f) in [("surface", 0usize), ("depth", 1), ("u", 2), ("w", 3)] {
                eprintln!("  {name}:");
                for k in fk.saturating_sub(3)..(fk + 4).min(size[1]) {
                    let row: Vec<String> = (fi.saturating_sub(3)..(fi + 4).min(size[0]))
                        .map(|i| {
                            let c = at(i, k);
                            let v = if f == 0 { c[0] + c[1] } else { c[f] };
                            format!("{v:7.2}")
                        })
                        .collect();
                    eprintln!("    {}", row.join(" "));
                }
            }
            if out.summary.max_speed > 3.0 * dump {
                panic!("dumped");
            }
        }
        if tick % 300 == 299 {
            eprintln!(
                "{:>5.1} s: max speed {:6.2} m/s, max depth {:6.2} m, volume {:9.1} m^3",
                f64::from(tick + 1) * DT,
                out.summary.max_speed,
                out.summary.max_depth,
                out.summary.volume_cells * 0.25
            );
        }
    }
}

/// The non-hydrostatic solve runs four Chebyshev iterations a dispatch on tiles in workgroup
/// memory (water_nh.wgsl `nh_block`); one dispatch an iteration (`REFERENCE`) must give the same
/// field bit for bit: a deep basin, and a beach of 100 x 36 columns (partial tiles) with dry land,
/// breaking and an open side.
#[test]
fn the_blocked_non_hydrostatic_solve_matches_one_dispatch_an_iteration() {
    use amoris_render::water_nh::REFERENCE;
    use std::sync::atomic::Ordering;
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("non-hydrostatic water requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("blocked solve validation: {e}")));
        let basin = {
            let start = field([64, 64], |i, k| {
                let d = ((i as f32 - 32.0).powi(2) + (k as f32 - 32.0).powi(2)) / 16.0;
                (-8.8, 8.8 + 0.3 * (-d).exp())
            });
            let mut s = step(0.5, 4, 0.0);
            s.calm_per_s = 0.0;
            s.physics.nonhydrostatic = true;
            (start, s)
        };
        let beach = {
            let cell = 0.5;
            let start = field([100, 36], |i, k| {
                let x = i as f32 * cell as f32;
                let bed = -4.0 + 5.0 * (x / 50.0);
                let d = ((x - 12.0).powi(2) + (k as f32 * 0.5 - 9.0).powi(2)) / 16.0;
                (bed, (0.6 * (-d).exp() - bed).max(0.0))
            });
            let mut s = step(cell, 4, 0.02);
            s.boundaries[0] = FieldBoundary::Outflow;
            s.physics.nonhydrostatic = true;
            s.physics.breaking = true;
            (start, s)
        };
        for (name, (start, s)) in [("basin", basin), ("beach", beach)] {
            let mut ends = vec![];
            for reference in [true, false] {
                REFERENCE.store(reference, Ordering::Relaxed);
                let mut run = Run::new(&gpu, start.clone());
                for _ in 0..180 {
                    run.step(&s, &[]);
                }
                ends.push(run.read(&s));
            }
            REFERENCE.store(false, Ordering::Relaxed);
            let differ = ends[0]
                .columns
                .iter()
                .zip(&ends[1].columns)
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(differ, 0, "{name}: {differ} columns differ after 3 s");
        }
    }
}
