//! The shore's numerics (water.md 61.1): the resident kernel the water regions run, against
//! analytic solutions of the shallow-water equations that shores and bores test a scheme with, and
//! against a shock-capturing finite-volume reference on the same grids: the central-upwind scheme
//! of A. Kurganov and G. Petrova ("A second-order well-balanced positivity preserving
//! central-upwind scheme for the Saint-Venant system", Commun. Math. Sci. 5(1), 2007), in f64 on
//! the CPU. Hydrostatic, frictionless, no breaking and no calm damping; one row of columns, at the
//! cell sizes regions use (0.375 and 0.5 m), the cases scaled to them as Froude scaling allows.
//!
//! - Ritter's dam break onto a dry bed: the front runs at 2 sqrt(g h0) (O. Delestre et al.,
//!   "SWASHES: a compilation of shallow water analytic solutions", IJNMF 72(3), 2013, 4.1.2).
//! - Stoker's dam break onto a wet bed: the bore's height and speed (SWASHES 4.1.1).
//! - A dam break onto a dry plane beach: the swash's tip leaves at 2 sqrt(g h0) and climbs
//!   ballistically to 2 h0 (R. E. Meyer and M. C. Shen; D. H. Peregrine and S. M. Williams,
//!   "Swash overtopping a truncated plane beach", JFM 440, 2001).
//! - A solitary wave running up a 1:19.85 beach: C. E. Synolakis's run-up law R/d = 2.831
//!   sqrt(cot b) (H/d)^(5/4) ("The runup of solitary waves", JFM 185, 1987).
//!
//! `--advection maccormack|conservative` runs the kernel with another transport of its face
//! velocities than the regions' hybrid one (water.md 61.1).
//!
//! cargo run --release -p amoris-render --example water_swash -- --out out/water-swash
use amoris_assets::water_field::{FieldBoundary, FieldCheckpoint, FieldPhysics, FieldStep};
use amoris_render::{
    Gpu,
    water_compute::{Advection, GpuWater},
};
use serde_json::{Value, json};

const G: f64 = 9.81;
/// Columns across the row: the kernel's smallest field.
const NZ: u32 = 4;

/// One test: the bed and the initial depth and velocity at a column centre (velocity at the
/// column's -x face for the kernel, at its centre for the reference).
struct Case {
    name: &'static str,
    nx: u32,
    dx: f64,
    dt: f64,
    end: f64,
    bed: fn(f64) -> f64,
    depth: fn(f64) -> f64,
    velocity: fn(f64) -> f64,
}

// ---- the resident kernel ------------------------------------------------------------------------

struct Kernel {
    field: GpuWater,
    case_nx: u32,
}

impl Kernel {
    fn new(gpu: &Gpu, case: &Case, advection: Advection) -> Kernel {
        let columns = (0..case.nx * NZ)
            .map(|n| {
                let x = (f64::from(n % case.nx) + 0.5) * case.dx;
                let face = x - 0.5 * case.dx;
                [
                    ((case.bed)(x) as f32).to_bits(),
                    ((case.depth)(x) as f32).to_bits(),
                    if n % case.nx == 0 {
                        0
                    } else {
                        ((case.velocity)(face) as f32).to_bits()
                    },
                    0,
                ]
            })
            .collect();
        let initial = FieldCheckpoint::new([case.nx, NZ], 0.0, columns);
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, initial.size).unwrap();
        field.set_advection(advection);
        field.upload_checkpoint(&initial).unwrap();
        Kernel {
            field,
            case_nx: case.nx,
        }
    }

    fn advance(&mut self, gpu: &Gpu, case: &Case, steps: u32) {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let mut left = steps;
        while left > 0 {
            let batch = left.min(8);
            self.encode(&mut encoder, case, batch);
            left -= batch;
        }
        gpu.queue.submit([encoder.finish()]);
    }

    fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, case: &Case, substeps: u32) {
        let step = FieldStep {
            exterior: vec![],
            boundaries: [FieldBoundary::Wall; 4],
            sea: None,
            cell_m: case.dx,
            substep_dt: case.dt,
            substeps,
            friction: 0.0,
            calm_per_s: 0.0,
            dry_below: 0.0,
            edits: Vec::new(),
            impulses: Vec::new(),
            displacements: Vec::new(),
            physics: FieldPhysics::default(),
            wind: None,
            rain_m: 0.0,
        };
        self.field.encode_transport(encoder, &step, None).unwrap();
    }

    /// Depth and cell-centre velocity along the middle row.
    fn row(&mut self) -> (Vec<f64>, Vec<f64>) {
        let cp = self.field.checkpoint_complete(0.0).unwrap();
        let nx = self.case_nx as usize;
        let start = nx * (NZ as usize / 2);
        let word = |n: usize, c: usize| f64::from(f32::from_bits(cp.columns[n][c]));
        let depth = (0..nx).map(|i| word(start + i, 1)).collect();
        let velocity = (0..nx)
            .map(|i| {
                let right = if i + 1 < nx {
                    word(start + i + 1, 2)
                } else {
                    0.0
                };
                0.5 * (word(start + i, 2) + right)
            })
            .collect();
        (depth, velocity)
    }
}

// ---- the central-upwind reference ---------------------------------------------------------------

/// Kurganov and Petrova's scheme: surface w = h + B and discharge q at cell centres, the bed
/// continuous and piecewise linear through its interface values, generalized-minmod slopes
/// (theta 1.3) with their positivity correction, desingularized velocities, central-upwind
/// fluxes, the well-balanced bed source, two-stage SSP Runge-Kutta; walls at both ends.
struct Reference {
    dx: f64,
    /// The bed at the n + 1 interfaces and at the n centres (their mean).
    edges: Vec<f64>,
    centres: Vec<f64>,
    w: Vec<f64>,
    q: Vec<f64>,
}

fn minmod3(a: f64, b: f64, c: f64) -> f64 {
    if a > 0.0 && b > 0.0 && c > 0.0 {
        a.min(b).min(c)
    } else if a < 0.0 && b < 0.0 && c < 0.0 {
        a.max(b).max(c)
    } else {
        0.0
    }
}

impl Reference {
    fn new(case: &Case) -> Reference {
        let n = case.nx as usize;
        let dx = case.dx;
        let edges: Vec<f64> = (0..=n).map(|i| (case.bed)(i as f64 * dx)).collect();
        let centres: Vec<f64> = (0..n).map(|i| 0.5 * (edges[i] + edges[i + 1])).collect();
        let w = (0..n)
            .map(|i| centres[i] + (case.depth)((i as f64 + 0.5) * dx))
            .collect();
        let q = (0..n)
            .map(|i| {
                let x = (i as f64 + 0.5) * dx;
                (case.depth)(x) * (case.velocity)(x)
            })
            .collect();
        Reference {
            dx,
            edges,
            centres,
            w,
            q,
        }
    }

    fn velocity(&self, h: f64, q: f64) -> f64 {
        // Kurganov and Petrova take eps as the fourth power of their grids' step, which at these
        // cells would stop water a third of a metre deep; a centimetre is the scale here, under
        // which velocities are tempered (a millimetre lets thin tips run past the exact ones).
        let eps = 1e-8;
        let h4 = h.powi(4);
        std::f64::consts::SQRT_2 * h * q / (h4 + h4.max(eps)).sqrt()
    }

    /// The right-hand side, and the largest wave speed.
    fn rate(&self, w: &[f64], q: &[f64]) -> (Vec<f64>, Vec<f64>, f64) {
        let n = w.len();
        let theta = 1.3;
        // Ghost cells mirror the walls: same surface, opposite discharge.
        let wi = |i: isize| -> f64 { w[i.clamp(0, n as isize - 1) as usize] };
        let qi = |i: isize| -> f64 {
            if i < 0 || i >= n as isize {
                -q[i.clamp(0, n as isize - 1) as usize]
            } else {
                q[i as usize]
            }
        };
        // Interface values: index j + 1/2 holds the left (minus) and right (plus) states.
        let mut wm = vec![0.0; n + 1];
        let mut wp = vec![0.0; n + 1];
        let mut qm = vec![0.0; n + 1];
        let mut qp = vec![0.0; n + 1];
        for j in 0..n {
            let ji = j as isize;
            let sw = minmod3(
                theta * (wi(ji) - wi(ji - 1)),
                0.5 * (wi(ji + 1) - wi(ji - 1)),
                theta * (wi(ji + 1) - wi(ji)),
            ) / self.dx;
            let sq = minmod3(
                theta * (qi(ji) - qi(ji - 1)),
                0.5 * (qi(ji + 1) - qi(ji - 1)),
                theta * (qi(ji + 1) - qi(ji)),
            ) / self.dx;
            let mut east = w[j] + 0.5 * self.dx * sw;
            let mut west = w[j] - 0.5 * self.dx * sw;
            // Positivity: neither interface value under the bed there.
            if east < self.edges[j + 1] {
                east = self.edges[j + 1];
                west = 2.0 * w[j] - self.edges[j + 1];
            } else if west < self.edges[j] {
                west = self.edges[j];
                east = 2.0 * w[j] - self.edges[j];
            }
            wm[j + 1] = east;
            wp[j] = west;
            qm[j + 1] = q[j] + 0.5 * self.dx * sq;
            qp[j] = q[j] - 0.5 * self.dx * sq;
        }
        // Walls: the outer states mirror the inner ones.
        wm[0] = wp[0];
        qm[0] = -qp[0];
        wp[n] = wm[n];
        qp[n] = -qm[n];
        let mut fh = vec![0.0; n + 1];
        let mut fq = vec![0.0; n + 1];
        let mut fastest = 0.0f64;
        for k in 0..=n {
            let b = self.edges[k];
            let (hm, hp) = ((wm[k] - b).max(0.0), (wp[k] - b).max(0.0));
            let (um, up) = (self.velocity(hm, qm[k]), self.velocity(hp, qp[k]));
            let (qm, qp) = (hm * um, hp * up);
            let ap = (up + (G * hp).sqrt()).max(um + (G * hm).sqrt()).max(0.0);
            let am = (up - (G * hp).sqrt()).min(um - (G * hm).sqrt()).min(0.0);
            fastest = fastest.max(ap).max(-am);
            if ap - am < 1e-12 {
                continue;
            }
            let flux = |h: f64, u: f64| (h * u, h * u * u + 0.5 * G * h * h);
            let (f1m, f2m) = flux(hm, um);
            let (f1p, f2p) = flux(hp, up);
            fh[k] = (ap * f1m - am * f1p + ap * am * (wp[k] - wm[k])) / (ap - am);
            fq[k] = (ap * f2m - am * f2p + ap * am * (qp - qm)) / (ap - am);
        }
        let mut dw = vec![0.0; n];
        let mut dq = vec![0.0; n];
        for j in 0..n {
            let h_east = (wm[j + 1] - self.edges[j + 1]).max(0.0);
            let h_west = (wp[j] - self.edges[j]).max(0.0);
            let source =
                -G * 0.5 * (h_east + h_west) * (self.edges[j + 1] - self.edges[j]) / self.dx;
            dw[j] = -(fh[j + 1] - fh[j]) / self.dx;
            dq[j] = -(fq[j + 1] - fq[j]) / self.dx + source;
        }
        (dw, dq, fastest)
    }

    fn advance(&mut self, until: f64, now: &mut f64) {
        while *now < until - 1e-12 {
            let (dw, dq, fastest) = self.rate(&self.w, &self.q);
            let dt = (0.45 * self.dx / fastest.max(1e-6)).min(until - *now);
            let w1: Vec<f64> = self.w.iter().zip(&dw).map(|(w, d)| w + dt * d).collect();
            let q1: Vec<f64> = self.q.iter().zip(&dq).map(|(q, d)| q + dt * d).collect();
            let (dw1, dq1, _) = self.rate(&w1, &q1);
            for j in 0..self.w.len() {
                self.w[j] = 0.5 * (self.w[j] + w1[j] + dt * dw1[j]);
                self.q[j] = 0.5 * (self.q[j] + q1[j] + dt * dq1[j]);
                if self.w[j] < self.centres[j] {
                    self.w[j] = self.centres[j];
                    self.q[j] = 0.0;
                }
            }
            *now += dt;
        }
    }

    fn row(&self) -> (Vec<f64>, Vec<f64>) {
        let depth: Vec<f64> = self
            .w
            .iter()
            .zip(&self.centres)
            .map(|(w, b)| (w - b).max(0.0))
            .collect();
        let velocity = depth
            .iter()
            .zip(&self.q)
            .map(|(h, q)| self.velocity(*h, *q))
            .collect();
        (depth, velocity)
    }
}

// ---- the cases ----------------------------------------------------------------------------------

const DAM_LENGTH: f64 = 200.0;
const H_LEFT: f64 = 4.0;
const H_RIGHT: f64 = 0.4;

fn flat(_: f64) -> f64 {
    0.0
}
fn still(_: f64) -> f64 {
    0.0
}
fn ritter_initial(x: f64) -> f64 {
    if x < 0.5 * DAM_LENGTH { H_LEFT } else { 0.0 }
}
fn stoker_initial(x: f64) -> f64 {
    if x < 0.5 * DAM_LENGTH {
        H_LEFT
    } else {
        H_RIGHT
    }
}
/// Ritter's depth at t, the dam at the middle.
fn ritter(x: f64, t: f64) -> f64 {
    let c0 = (G * H_LEFT).sqrt();
    let s = (x - 0.5 * DAM_LENGTH) / t;
    if s <= -c0 {
        H_LEFT
    } else if s <= 2.0 * c0 {
        4.0 / (9.0 * G) * (c0 - 0.5 * s).powi(2)
    } else {
        0.0
    }
}
/// The middle state of Stoker's solution: cm = sqrt(g hm) solves
/// -8 g hR cm^2 (sqrt(g hL) - cm)^2 + (cm^2 - g hR)^2 (cm^2 + g hR) = 0.
fn stoker_middle() -> f64 {
    let (cl, gr) = ((G * H_LEFT).sqrt(), G * H_RIGHT);
    let f = |c: f64| -8.0 * gr * c * c * (cl - c).powi(2) + (c * c - gr).powi(2) * (c * c + gr);
    let (mut lo, mut hi) = (gr.sqrt(), cl);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if f(lo) * f(mid) <= 0.0 {
            hi = mid
        } else {
            lo = mid
        }
    }
    0.5 * (lo + hi)
}
fn stoker(x: f64, t: f64) -> f64 {
    let cl = (G * H_LEFT).sqrt();
    let cm = stoker_middle();
    let s = (x - 0.5 * DAM_LENGTH) / t;
    let bore = 2.0 * cm * cm * (cl - cm) / (cm * cm - G * H_RIGHT);
    if s <= -cl {
        H_LEFT
    } else if s <= 2.0 * cl - 3.0 * cm {
        4.0 / (9.0 * G) * (cl - 0.5 * s).powi(2)
    } else if s <= bore {
        cm * cm / G
    } else {
        H_RIGHT
    }
}

/// The swash case: a reservoir 2 m deep on a flat bed, its gate at x = 80 m, where a 1:10 beach
/// begins.
const GATE: f64 = 80.0;
const RESERVOIR: f64 = 2.0;
const BEACH: f64 = 0.1;
fn swash_bed(x: f64) -> f64 {
    BEACH * (x - GATE).max(0.0)
}
fn swash_initial(x: f64) -> f64 {
    if x < GATE { RESERVOIR } else { 0.0 }
}

/// Synolakis's case over 4 m of water: a 1:19.85 beach whose toe is 79.4 m seaward of the still
/// shoreline at x = 240 m, and a solitary wave 0.0185 times the depth high, centred where its
/// front reaches the toe.
const SHORE: f64 = 240.0;
const SLOPE: f64 = 1.0 / 19.85;
const DEPTH: f64 = 4.0;
const SOLITARY: f64 = 0.0185 * DEPTH;
fn solitary_bed(x: f64) -> f64 {
    (SLOPE * (x - SHORE)).max(-DEPTH)
}
fn solitary_k() -> f64 {
    (3.0 * SOLITARY / (4.0 * DEPTH.powi(3))).sqrt()
}
fn solitary_centre() -> f64 {
    let toe = SHORE - DEPTH / SLOPE;
    toe - (1.0 / 0.05f64.sqrt()).acosh() / solitary_k()
}
fn solitary_eta(x: f64) -> f64 {
    let s = 1.0 / (solitary_k() * (x - solitary_centre())).cosh();
    SOLITARY * s * s
}
fn solitary_depth(x: f64) -> f64 {
    (solitary_eta(x) - solitary_bed(x)).max(0.0)
}
fn solitary_velocity(x: f64) -> f64 {
    // Toward the shore at +x: u = c eta / (d + eta), c = sqrt(g (d + H)).
    let eta = solitary_eta(x);
    if solitary_bed(x) > -DEPTH + 1e-3 {
        return 0.0;
    }
    (G * (DEPTH + SOLITARY)).sqrt() * eta / (DEPTH + eta)
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "ritter",
            nx: 512,
            dx: DAM_LENGTH / 512.0,
            dt: 0.004,
            end: 6.0,
            bed: flat,
            depth: ritter_initial,
            velocity: still,
        },
        Case {
            name: "stoker",
            nx: 512,
            dx: DAM_LENGTH / 512.0,
            dt: 0.004,
            end: 6.0,
            bed: flat,
            depth: stoker_initial,
            velocity: still,
        },
        Case {
            name: "swash",
            nx: 384,
            dx: 0.375,
            dt: 0.004,
            end: 12.0,
            bed: swash_bed,
            depth: swash_initial,
            velocity: still,
        },
        Case {
            name: "solitary",
            nx: 512,
            dx: 0.5,
            dt: 0.01,
            end: 42.0,
            bed: solitary_bed,
            depth: solitary_depth,
            velocity: solitary_velocity,
        },
    ]
}

// ---- measures -----------------------------------------------------------------------------------

/// The landward-most wet column's position and its bed's height.
fn tip(case: &Case, depth: &[f64], threshold: f64) -> (f64, f64) {
    let last = depth.iter().rposition(|h| *h > threshold).unwrap_or(0);
    let x = (last as f64 + 0.5) * case.dx;
    (x, (case.bed)(x))
}

fn l1(case: &Case, depth: &[f64], exact: impl Fn(f64) -> f64) -> f64 {
    depth
        .iter()
        .enumerate()
        .map(|(i, h)| (h - exact((i as f64 + 0.5) * case.dx)).abs())
        .sum::<f64>()
        * case.dx
}

/// The bore's position: where the depth first falls under the mean of the middle state and the
/// still water ahead, seaward of the front.
fn bore(case: &Case, depth: &[f64]) -> (f64, f64) {
    let middle = stoker_middle().powi(2) / G;
    let level = 0.5 * (middle + H_RIGHT);
    let i = depth.iter().rposition(|h| *h > level).unwrap_or(0);
    let x = (i as f64 + 0.5) * case.dx;
    // The plateau behind it: the median over the 6 m behind the bore.
    let back = (6.0 / case.dx) as usize;
    let mut plateau: Vec<f64> = depth[i.saturating_sub(back)..=i].to_vec();
    plateau.sort_by(f64::total_cmp);
    (x, plateau[plateau.len() / 2])
}

fn run(gpu: &Gpu, case: &Case, advection: Advection) -> Value {
    let samples = 60u32;
    let sample_dt = case.end / f64::from(samples);
    let steps = (sample_dt / case.dt).round() as u32;
    assert!(
        (f64::from(steps) * case.dt - sample_dt).abs() < 1e-9,
        "{}",
        case.name
    );
    let mut kernel = Kernel::new(gpu, case, advection);
    let mut reference = Reference::new(case);
    let mut now = 0.0;
    let volume = |d: &[f64]| d.iter().sum::<f64>() * case.dx;
    let v0 = volume(&kernel.row().0);
    let mut rows = Vec::new();
    let mut highest = [f64::MIN; 2];
    for sample in 1..=samples {
        kernel.advance(gpu, case, steps);
        let t = f64::from(sample) * sample_dt;
        reference.advance(t, &mut now);
        let (kd, ku) = kernel.row();
        let (rd, ru) = reference.row();
        let mut row = json!({"t": t});
        if sample % 10 == 0 {
            row["profile"] = json!({"kernel": [kd, ku], "reference": [rd, ru]});
        }
        let threshold = 1e-3;
        let (kx, kz) = tip(case, &kd, threshold);
        let (rx, rz) = tip(case, &rd, threshold);
        highest[0] = highest[0].max(kz);
        highest[1] = highest[1].max(rz);
        row["tip"] = json!({"kernel": [kx, kz], "reference": [rx, rz]});
        match case.name {
            "ritter" => {
                let exact = 0.5 * DAM_LENGTH + 2.0 * (G * H_LEFT).sqrt() * t;
                row["front_exact"] = json!(exact);
                row["l1"] = json!({"kernel": l1(case, &kd, |x| ritter(x, t)),
                    "reference": l1(case, &rd, |x| ritter(x, t))});
            }
            "stoker" => {
                let cm = stoker_middle();
                let cl = (G * H_LEFT).sqrt();
                let speed = 2.0 * cm * cm * (cl - cm) / (cm * cm - G * H_RIGHT);
                row["bore_exact"] = json!([0.5 * DAM_LENGTH + speed * t, cm * cm / G]);
                row["bore"] = json!({"kernel": bore(case, &kd), "reference": bore(case, &rd)});
                row["l1"] = json!({"kernel": l1(case, &kd, |x| stoker(x, t)),
                    "reference": l1(case, &rd, |x| stoker(x, t))});
            }
            "swash" => {
                // The ballistic tip: along the beach at 2 sqrt(g h0) less g sin(b) t^2 / 2, the
                // height on the beach its distance times the slope.
                let along = 2.0 * (G * RESERVOIR).sqrt() * t
                    - 0.5 * G * (BEACH / (1.0 + BEACH * BEACH).sqrt()) * t * t;
                let height = along.max(0.0) * BEACH / (1.0 + BEACH * BEACH).sqrt();
                row["tip_exact_height"] = json!(height);
            }
            _ => {}
        }
        row["volume_error"] = json!(volume(&kd) - v0);
        rows.push(row);
    }
    let last = rows.last().cloned().unwrap();
    let mut summary = json!({"case": case.name, "nx": case.nx, "dx": case.dx, "dt": case.dt,
        "highest_tip": {"kernel": highest[0], "reference": highest[1]}, "last": last});
    match case.name {
        "swash" => summary["runup_exact"] = json!(2.0 * RESERVOIR),
        "solitary" => {
            summary["runup_exact"] =
                json!(DEPTH * 2.831 * (1.0 / SLOPE).sqrt() * (SOLITARY / DEPTH).powf(1.25));
        }
        _ => {}
    }
    summary["rows"] = Value::Array(rows);
    summary
}

fn main() {
    amoris_render::stderr_log::init();
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| {
        args.windows(2)
            .find(|a| a[0] == name)
            .map(|a| a[1].as_str())
    };
    let out = arg("--out").unwrap_or("out/water-swash");
    let advection = match arg("--advection") {
        Some("maccormack") => Advection::MacCormack,
        Some("conservative") => Advection::Conservative,
        None | Some("hybrid") => Advection::Hybrid,
        Some(other) => panic!("no advection {other}: maccormack, conservative or hybrid"),
    };
    std::fs::create_dir_all(out).unwrap();
    let gpu = Gpu::headless(amoris_render::host_backend()).expect("the swash study needs a GPU");
    let mut all = Vec::new();
    for case in cases() {
        let row = run(&gpu, &case, advection);
        let last = &row["last"];
        println!(
            "{}: highest tip kernel {:.4} m, reference {:.4} m{}; last {}",
            case.name,
            row["highest_tip"]["kernel"].as_f64().unwrap(),
            row["highest_tip"]["reference"].as_f64().unwrap(),
            row.get("runup_exact").map_or(String::new(), |v| format!(
                ", exact {:.4} m",
                v.as_f64().unwrap()
            )),
            last
        );
        all.push(row);
    }
    std::fs::write(
        std::path::Path::new(out).join("swash.json"),
        serde_json::to_vec_pretty(&json!({"adapter": gpu.info.name,
            "backend": gpu.backend_name(), "advection": format!("{advection:?}"), "cases": all}))
        .unwrap(),
    )
    .unwrap();
}
