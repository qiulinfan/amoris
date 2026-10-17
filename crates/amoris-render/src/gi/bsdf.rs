//! Independent f64 reference for the surface integrator's sampling contract.
//!
//! This is not the renderer's CPU transport path. Tests compare numerical integration against
//! independent Monte Carlo samples, Fresnel/Snell identities and Russian-roulette compensation.
//! Equation references: Heitz, JCGT 7(4), 2018 (GGX VNDF); PBRT 4e §9.5 (dielectric Jacobian).

use glam::DVec3;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug)]
pub struct Bsdf {
    pub base: DVec3,
    pub metallic: f64,
    pub roughness: f64,
    pub transmission: f64,
    /// Transmitted medium's IOR divided by incident medium's IOR.
    pub eta: f64,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Evaluation {
    /// BSDF times absolute incoming cosine, in radiance transport mode.
    pub value: DVec3,
    pub pdf: f64,
}
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub direction: DVec3,
    pub weight: DVec3,
    pub pdf: f64,
    pub delta: bool,
    /// Compensates IOR compression when choosing Russian-roulette survival probability.
    pub eta_scale: f64,
}

impl Default for Sample {
    fn default() -> Self {
        Self {
            direction: DVec3::ZERO,
            weight: DVec3::ZERO,
            pdf: 0.0,
            delta: false,
            eta_scale: 1.0,
        }
    }
}

pub fn fresnel(cosine: f64, eta: f64) -> f64 {
    let c = cosine.abs().clamp(0.0, 1.0);
    let sin2_t = (1.0 - c * c).max(0.0) / (eta * eta);
    if sin2_t >= 1.0 {
        return 1.0;
    }
    let ct = (1.0 - sin2_t).sqrt();
    let rs = (c - eta * ct) / (c + eta * ct).max(1e-30);
    let rp = (eta * c - ct) / (eta * c + ct).max(1e-30);
    (rs * rs + rp * rp) * 0.5
}
fn schlick(f0: DVec3, cosine: f64) -> DVec3 {
    f0 + (DVec3::ONE - f0) * (1.0 - cosine.abs().clamp(0.0, 1.0)).powi(5)
}
fn luminance(c: DVec3) -> f64 {
    c.dot(DVec3::new(0.2126, 0.7152, 0.0722))
}
fn ggx_d(h: DVec3, alpha: f64) -> f64 {
    let a2 = alpha * alpha;
    let d = h.x * h.x + h.y * h.y + a2 * h.z * h.z;
    a2 / (PI * d * d).max(1e-30)
}
fn lambda(w: DVec3, alpha: f64) -> f64 {
    if w.z * w.z < 1e-30 {
        return 1e30;
    }
    ((1.0 + alpha * alpha * (1.0 - w.z * w.z).max(0.0) / (w.z * w.z)).sqrt() - 1.0) * 0.5
}
fn h_pdf(wo: DVec3, h: DVec3, alpha: f64) -> f64 {
    ggx_d(h, alpha) * wo.dot(h).abs() / (wo.z * (1.0 + lambda(wo, alpha))).max(1e-30)
}

pub fn visible_normal(wo: DVec3, alpha: f64, u: [f64; 2]) -> DVec3 {
    let v = DVec3::new(alpha * wo.x, alpha * wo.y, wo.z).normalize();
    let lensq = v.x * v.x + v.y * v.y;
    let t1 = if lensq > 1e-30 {
        DVec3::new(-v.y, v.x, 0.0) / lensq.sqrt()
    } else {
        DVec3::X
    };
    let t2 = v.cross(t1);
    let radius = u[0].sqrt();
    let angle = 2.0 * PI * u[1];
    let diskx = radius * angle.cos();
    let mut disky = radius * angle.sin();
    let s = (1.0 + v.z) * 0.5;
    disky = (1.0 - s) * (1.0 - diskx * diskx).max(0.0).sqrt() + s * disky;
    let nh = diskx * t1 + disky * t2 + (1.0 - diskx * diskx - disky * disky).max(0.0).sqrt() * v;
    DVec3::new(alpha * nh.x, alpha * nh.y, nh.z.max(0.0)).normalize()
}

impl Bsdf {
    fn f0(self) -> DVec3 {
        let r = (self.eta - 1.0) / (self.eta + 1.0);
        DVec3::splat(r * r).lerp(self.base, self.metallic)
    }
    fn glass(self) -> f64 {
        (1.0 - self.metallic) * self.transmission
    }
    fn spec_probability(self) -> f64 {
        if self.metallic >= 0.999999 {
            return 1.0;
        }
        let s = luminance(self.f0());
        let d = luminance(self.base) * (1.0 - self.metallic);
        (s / (s + d).max(1e-8)).clamp(0.1, 0.9)
    }
    pub fn evaluate(self, wo: DVec3, wi: DVec3) -> Evaluation {
        if wo.z <= 0.0 || wi.z.abs() < 1e-12 {
            return Evaluation::default();
        }
        let glass = self.glass();
        let opaque = 1.0 - glass;
        let spec = self.spec_probability();
        let smooth = self.roughness <= 0.0001;
        let alpha = (self.roughness * self.roughness).max(0.002);
        if wi.z > 0.0 {
            let h = (wo + wi).normalize();
            let f = schlick(self.f0(), if smooth { wo.z } else { wo.dot(h) });
            let mut value =
                opaque * self.base * (1.0 - self.metallic) * (DVec3::ONE - f) * wi.z / PI;
            let mut pdf = opaque * (1.0 - spec) * wi.z / PI;
            if !smooth {
                let rp = h_pdf(wo, h, alpha) / (4.0 * wo.dot(h).abs()).max(1e-30);
                let fr = fresnel(wo.dot(h), self.eta);
                let g = 1.0 / (1.0 + lambda(wo, alpha) + lambda(wi, alpha));
                value +=
                    (opaque * f + glass * DVec3::splat(fr)) * ggx_d(h, alpha) * g / (4.0 * wo.z);
                pdf += (opaque * spec + glass * fr) * rp;
            }
            return Evaluation { value, pdf };
        }
        if smooth || glass <= 0.0 || (self.eta - 1.0).abs() < 1e-6 {
            return Evaluation::default();
        }
        let mut h = (wo + self.eta * wi).normalize();
        if h.z < 0.0 {
            h = -h;
        }
        let oh = wo.dot(h);
        let ih = wi.dot(h);
        if oh <= 0.0 || ih >= 0.0 {
            return Evaluation::default();
        }
        let fr = fresnel(oh, self.eta);
        let denom2 = (ih + oh / self.eta).powi(2);
        if denom2 < 1e-30 {
            return Evaluation::default();
        }
        let g = 1.0 / (1.0 + lambda(wo, alpha) + lambda(wi, alpha));
        Evaluation {
            value: glass * self.base * (1.0 - fr) * ggx_d(h, alpha) * g * (ih * oh).abs()
                / (wo.z * denom2 * self.eta * self.eta),
            pdf: glass * (1.0 - fr) * h_pdf(wo, h, alpha) * ih.abs() / denom2,
        }
    }
    pub fn sample(self, wo: DVec3, u: [f64; 4]) -> Sample {
        let empty = Sample::default();
        if wo.z <= 0.0 {
            return empty;
        }
        let glass = self.glass();
        let opaque = 1.0 - glass;
        let spec = self.spec_probability();
        let choose_glass = u[0] < glass;
        let smooth = self.roughness <= 0.0001;
        let mut transmitted = false;
        let wi;
        if choose_glass || u[1] < spec {
            let h = if smooth {
                DVec3::Z
            } else {
                visible_normal(
                    wo,
                    (self.roughness * self.roughness).max(0.002),
                    [u[2], u[3]],
                )
            };
            let oh = wo.dot(h);
            let fr = fresnel(oh, self.eta);
            if choose_glass && u[1] >= fr {
                let discriminant = 1.0 - (1.0 - oh * oh).max(0.0) / (self.eta * self.eta);
                if discriminant <= 0.0 {
                    return empty;
                }
                wi = -wo / self.eta + h * (oh / self.eta - discriminant.sqrt());
                if wi.z >= 0.0 {
                    return empty;
                }
                transmitted = true;
            } else {
                wi = -wo + 2.0 * oh * h;
                if wi.z <= 0.0 {
                    return empty;
                }
            }
            if smooth || (choose_glass && (self.eta - 1.0).abs() < 1e-6) {
                if transmitted {
                    return Sample {
                        direction: wi,
                        weight: self.base / (self.eta * self.eta),
                        pdf: glass * (1.0 - fr),
                        delta: true,
                        eta_scale: self.eta * self.eta,
                    };
                }
                let pdf = opaque * spec + glass * fr;
                return Sample {
                    direction: wi,
                    weight: (opaque * schlick(self.f0(), wo.z) + glass * DVec3::splat(fr))
                        / pdf.max(1e-30),
                    pdf,
                    delta: true,
                    eta_scale: 1.0,
                };
            }
        } else {
            let radius = u[2].sqrt();
            let angle = 2.0 * PI * u[3];
            wi = DVec3::new(
                radius * angle.cos(),
                radius * angle.sin(),
                (1.0 - u[2]).max(0.0).sqrt(),
            );
        }
        let result = self.evaluate(wo, wi);
        if result.pdf <= 1e-30 {
            return empty;
        }
        Sample {
            direction: wi,
            weight: result.value / result.pdf,
            pdf: result.pdf,
            delta: false,
            eta_scale: if transmitted {
                self.eta * self.eta
            } else {
                1.0
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
        }
        fn four(&mut self) -> [f64; 4] {
            [self.next(), self.next(), self.next(), self.next()]
        }
    }
    fn material(metallic: f64, transmission: f64, roughness: f64, eta: f64) -> Bsdf {
        Bsdf {
            base: DVec3::new(0.7, 0.5, 0.3),
            metallic,
            roughness,
            transmission,
            eta,
        }
    }

    #[test]
    fn fresnel_snell_and_total_internal_reflection() {
        assert!((fresnel(1.0, 1.5) - 0.04).abs() < 1e-12);
        assert!(fresnel(0.0, 1.5) > 0.999999);
        assert_eq!(fresnel(0.6, 1.0 / 1.5), 1.0);
        let wo = DVec3::new(0.6, 0.0, 0.8);
        let glass = material(0.0, 1.0, 0.0, 1.5);
        let s = glass.sample(wo, [0.0, 0.99, 0.5, 0.5]);
        assert!(s.delta && s.direction.z < 0.0);
        assert!((s.direction.x.abs() * glass.eta - wo.x.abs()).abs() < 1e-12);
        assert!((s.weight.x * s.eta_scale - glass.base.x).abs() < 1e-12);
        // Enter and leave a planar air/glass slab: radiance compression cancels exactly.
        let exit = Bsdf {
            eta: 1.0 / 1.5,
            base: DVec3::ONE,
            ..glass
        };
        let back = exit.sample(-s.direction, [0.0, 0.99, 0.0, 0.0]);
        assert!((back.direction + wo).length() < 1e-12);
        assert!((back.weight.x * s.weight.x - glass.base.x).abs() < 1e-12);
    }

    #[test]
    fn smooth_reflection_and_transmission_are_energy_bounded() {
        let wo = DVec3::new(0.3, 0.0, (1.0f64 - 0.09).sqrt());
        let b = material(0.0, 1.0, 0.0, 1.5);
        let fr = fresnel(wo.z, b.eta);
        let reflected = b.sample(wo, [0.0, 0.0, 0.0, 0.0]);
        let transmitted = b.sample(wo, [0.0, 0.999, 0.0, 0.0]);
        let energy = reflected.weight * reflected.pdf
            + transmitted.weight * transmitted.pdf * transmitted.eta_scale;
        assert!((energy - (DVec3::splat(fr) + b.base * (1.0 - fr))).length() < 1e-12);
        assert!(energy.max_element() <= 1.0);
    }

    #[test]
    fn vndf_sampling_matches_direction_pdf_and_transport_integral() {
        // Independent uniform-direction integration vs visible-normal/cosine sampling.
        // Include rough transmission on both entering and leaving interfaces and a mixed lobe.
        let mut rng = Rng(0x689ed61449d1);
        let wo = DVec3::new(0.4, 0.0, (1.0f64 - 0.16).sqrt());
        for b in [
            material(1.0, 0.0, 0.75, 1.5),
            material(0.0, 0.0, 0.75, 1.5),
            material(0.0, 1.0, 0.75, 1.5),
            material(0.0, 1.0, 0.75, 1.0 / 1.5),
            material(0.15, 0.65, 0.75, 1.5),
        ] {
            let n = 240_000u32;
            let mut integral = DVec3::ZERO;
            let mut pdf_mass = 0.0;
            let mut estimate = DVec3::ZERO;
            let mut accepted = 0.0;
            let mut observed_bins = [0u32; 8];
            let mut integrated_bins = [0.0f64; 8];
            for _ in 0..n {
                let z = 2.0 * rng.next() - 1.0;
                let phi = 2.0 * PI * rng.next();
                let r = (1.0 - z * z).sqrt();
                let wi = DVec3::new(r * phi.cos(), r * phi.sin(), z);
                let e = b.evaluate(wo, wi);
                integral += e.value * (4.0 * PI / n as f64);
                pdf_mass += e.pdf * (4.0 * PI / n as f64);
                integrated_bins[((z + 1.0) * 4.0).floor().min(7.0) as usize] +=
                    e.pdf * (4.0 * PI / n as f64);
                let s = b.sample(wo, rng.four());
                if s.pdf > 0.0 {
                    assert!(s.weight.is_finite() && s.weight.min_element() >= 0.0);
                    assert!((s.direction.length() - 1.0).abs() < 1e-10);
                    let check = b.evaluate(wo, s.direction);
                    assert!((check.pdf - s.pdf).abs() < 1e-12);
                    estimate += s.weight / n as f64;
                    accepted += 1.0 / n as f64;
                    observed_bins[((s.direction.z + 1.0) * 4.0).floor().min(7.0) as usize] += 1;
                }
            }
            assert!(
                (estimate - integral).length() < 0.025,
                "transport mismatch {b:?}: sampled {estimate}, quadrature {integral}"
            );
            assert!(
                (accepted - pdf_mass).abs() < 0.025,
                "PDF mass mismatch {b:?}: {accepted} vs {pdf_mass}"
            );
            for i in 0..8 {
                assert!(
                    (observed_bins[i] as f64 / n as f64 - integrated_bins[i]).abs() < 0.018,
                    "direction histogram mismatch {b:?}, bin {i}"
                );
            }
        }
    }

    #[test]
    fn roulette_compensation_preserves_expected_brightness() {
        let mut rng = Rng(0x4322acde1234);
        for survival in [0.05, 0.2, 0.6, 0.95] {
            let mut mean = 0.0;
            for i in 0..300_000 {
                if (i as f64 + rng.next()) / 300_000.0 < survival {
                    mean += 0.37 / survival / 300_000.0;
                }
            }
            assert!(
                (mean - 0.37).abs() < 0.0001,
                "roulette darkens: {survival} => {mean}"
            );
        }
    }
}
