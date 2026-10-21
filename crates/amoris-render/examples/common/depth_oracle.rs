//! Independent f64 shared-face flux graph for the isolated linear-depth reconstruction.
use amoris_assets::water_field::FieldCheckpoint;
use amoris_render::Gpu;
use serde_json::json;
pub fn reference(size: [u32; 2], h: &[f64], v: &[[f32; 2]], dx: f64, dt: f64) -> Vec<f64> {
    let [nx, nz] = size.map(|n| n as usize);
    let at = |i, k| i + nx * k;
    let limiter = |a: f64, b: f64| {
        if a * b <= 0.0 {
            0.0
        } else {
            a.signum() * a.abs().min(b.abs())
        }
    };
    let sx = (0..h.len())
        .map(|n| {
            if n % nx > 0 && n % nx + 1 < nx {
                limiter(h[n] - h[n - 1], h[n + 1] - h[n])
            } else {
                0.0
            }
        })
        .collect::<Vec<_>>();
    let sz = (0..h.len())
        .map(|n| {
            if n / nx > 0 && n / nx + 1 < nz {
                limiter(h[n] - h[n - nx], h[n + nx] - h[n])
            } else {
                0.0
            }
        })
        .collect::<Vec<_>>();
    let mut qx = vec![0.0; (nx + 1) * nz];
    let mut qz = vec![0.0; nx * (nz + 1)];
    for k in 0..nz {
        for i in 1..nx {
            let a = at(i - 1, k);
            let b = at(i, k);
            let u = f64::from(v[b][0]);
            qx[i + (nx + 1) * k] = u * if u > 0.0 {
                h[a] + 0.5 * sx[a]
            } else {
                h[b] - 0.5 * sx[b]
            };
        }
    }
    for k in 1..nz {
        for i in 0..nx {
            let a = at(i, k - 1);
            let b = at(i, k);
            let u = f64::from(v[b][1]);
            qz[i + nx * k] = u * if u > 0.0 {
                h[a] + 0.5 * sz[a]
            } else {
                h[b] - 0.5 * sz[b]
            };
        }
    }
    let mut budgets = vec![1.0; h.len()];
    for k in 0..nz {
        for i in 0..nx {
            let n = at(i, k);
            let out = (-qx[i + (nx + 1) * k]).max(0.0)
                + qx[i + 1 + (nx + 1) * k].max(0.0)
                + (-qz[i + nx * k]).max(0.0)
                + qz[i + nx * (k + 1)].max(0.0);
            if out * dt / dx > h[n] {
                budgets[n] = h[n] * dx / (dt * out);
            }
        }
    }
    for k in 0..nz {
        for i in 1..nx {
            let q = &mut qx[i + (nx + 1) * k];
            *q *= budgets[at(if *q > 0.0 { i - 1 } else { i }, k)];
        }
    }
    for k in 1..nz {
        for i in 0..nx {
            let q = &mut qz[i + nx * k];
            *q *= budgets[at(i, if *q > 0.0 { k - 1 } else { k })];
        }
    }
    (0..h.len())
        .map(|n| {
            let i = n % nx;
            let k = n / nx;
            h[n] + dt / dx
                * (qx[i + (nx + 1) * k] - qx[i + 1 + (nx + 1) * k] + qz[i + nx * k]
                    - qz[i + nx * (k + 1)])
        })
        .collect()
}
pub fn fixtures(gpu: &Gpu) -> serde_json::Value {
    let mut cases = Vec::new();
    let size = [32, 16];
    let count = (size[0] * size[1]) as usize;
    for kind in 0..6 {
        let mut seed = 17u32;
        let mut random = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((seed >> 8) as f32) / 16777216.0
        };
        let mut h = Vec::new();
        let mut v = Vec::new();
        for n in 0..count {
            let i = n % 32;
            let k = n / 32;
            let value = match kind {
                0 | 1 => 0.25 + i as f32 * 0.03125,
                2 | 3 => 0.25 + k as f32 * 0.0625,
                4 => {
                    if n % 3 == 0 {
                        0.0
                    } else {
                        random() * 2.0
                    }
                }
                _ => 0.0,
            };
            h.push(value);
            let mut velocity = match kind {
                0 => [0.5, 0.0],
                1 => [-0.5, 0.0],
                2 => [0.0, 0.5],
                3 => [0.0, -0.5],
                _ => [
                    (random() * 2.0 - 1.0) * 300.0,
                    (random() * 2.0 - 1.0) * 300.0,
                ],
            };
            if i == 0 {
                velocity[0] = 0.0;
            }
            if k == 0 {
                velocity[1] = 0.0;
            }
            v.push(velocity);
        }
        let dx = 0.25f32;
        let dt = if kind < 4 { 0.125f32 } else { 0.2 };
        let initial = FieldCheckpoint::new(
            size,
            0.0,
            h.iter()
                .enumerate()
                .map(|(n, h)| [((n % 7) as f32 * 0.125).to_bits(), h.to_bits(), 0, 0])
                .collect(),
        );
        let expected = reference(
            size,
            &h.iter().map(|h| f64::from(*h)).collect::<Vec<_>>(),
            &v,
            f64::from(dx),
            f64::from(dt),
        );
        let mut field = super::depth_transport::Field::new(gpu, &initial, "muscl");
        field.transport(dx, dt, &v);
        let actual = field.checkpoint();
        let mut max_error = 0.0f64;
        for (n, (c, e)) in actual.columns.iter().zip(&expected).enumerate() {
            assert_eq!(c[0], initial.columns[n][0]);
            assert_eq!(c[2], v[n][0].to_bits());
            assert_eq!(c[3], v[n][1].to_bits());
            let depth = f64::from(f32::from_bits(c[1]));
            assert!(depth >= 0.0 && depth.is_finite());
            max_error = max_error.max((depth - e).abs());
        }
        let mass = actual
            .columns
            .iter()
            .map(|c| f64::from(f32::from_bits(c[1])))
            .sum::<f64>()
            - h.iter().map(|h| f64::from(*h)).sum::<f64>();
        assert!(max_error < 3e-6, "depth graph case={kind}: {max_error}");
        assert!(mass.abs() * f64::from(dx * dx) < 5e-6, "mass {mass}");
        cases.push(json!({"case":kind,"size":size,"dx":dx,"dt":dt,"initial":initial.columns,"velocity":v,"expected_depth":expected,"max_error":max_error,"mass_error_m3":mass*f64::from(dx*dx)}));
    }
    json!({"scope":"isolated limited linear depth flux versus independently assembled f64 face graph; four directions, super-CFL donor budgets, empty field","cases":cases})
}
