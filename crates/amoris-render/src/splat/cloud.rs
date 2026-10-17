//! A splat cloud on the CPU, packed the way the GPU reads it (docs/spec/splats.md 3).
//!
//! Each Gaussian is 32 bytes: position `f32x3`, rotation as a smallest-three quaternion in one
//! `u32` (2-bit index of the dropped largest component, 3 x 10 bits), scale and opacity as four
//! `f16`, base color as three `f16`. The view-dependent spherical-harmonic terms (degree 1 to 3)
//! live in a separate stream of `f16` pairs, coefficient-major with RGB interleaved, so a cloud
//! without them costs nothing extra.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// The zeroth spherical-harmonic band's constant: `color = 0.5 + SH_C0 * f_dc`.
pub const SH_C0: f32 = 0.282_094_8;

/// One Gaussian as the GPU reads it (matches `Splat` in splat_preprocess.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct PackedSplat {
    pub position: [f32; 3],
    /// Smallest-three quaternion.
    pub rotation: u32,
    /// `f16` pairs: (scale x, scale y), (scale z, opacity).
    pub scale_opacity: [u32; 2],
    /// `f16` pairs: (r, g), (b, 0). Display-encoded base color, `0.5 + SH_C0 * f_dc`.
    pub color: [u32; 2],
}

/// One Gaussian in plain numbers: what a file decodes to and what a generator produces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawSplat {
    pub position: [f32; 3],
    /// Standard deviations along the local axes (linear, not log).
    pub scale: [f32; 3],
    /// Unit quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Display-encoded (sRGB-like) base color, nominally 0..1.
    pub color: [f32; 3],
    /// 0..1 (after the sigmoid).
    pub opacity: f32,
}

/// The coefficients of the bands above zero: 3 for degree 1, 8 for 2, 15 for 3.
pub fn sh_coeffs(degree: u32) -> usize {
    ((degree + 1) * (degree + 1) - 1) as usize
}

/// `u32` words per splat in the SH stream: three `f16` per coefficient, two per word.
pub fn sh_words(degree: u32) -> usize {
    (sh_coeffs(degree) * 3).div_ceil(2)
}

/// A packed cloud.
#[derive(Clone, Debug, Default)]
pub struct SplatCloud {
    pub splats: Vec<PackedSplat>,
    /// 0 to 3.
    pub sh_degree: u32,
    /// `sh_words(sh_degree)` words per splat.
    pub sh: Vec<u32>,
    /// Local-space bounds of the centers, grown by three standard deviations of the largest splat.
    pub bounds: (Vec3, Vec3),
}

impl SplatCloud {
    pub fn len(&self) -> usize {
        self.splats.len()
    }

    pub fn is_empty(&self) -> bool {
        self.splats.is_empty()
    }

    /// Packs `raw`. `sh` holds `sh_coeffs(sh_degree)` RGB triples per splat, coefficient-major
    /// (`[c1.r, c1.g, c1.b, c2.r, ...]`), or is empty when `sh_degree` is 0.
    pub fn from_raw(raw: &[RawSplat], sh_degree: u32, sh: &[f32]) -> SplatCloud {
        let degree = sh_degree.min(3);
        let per = sh_coeffs(degree) * 3;
        assert!(
            degree == 0 || sh.len() == raw.len() * per,
            "SH stream length"
        );
        let words = sh_words(degree);
        let mut packed_sh = Vec::with_capacity(raw.len() * words);
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        let mut largest = 0.0f32;
        let splats = raw
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let p = Vec3::from(s.position);
                if p.is_finite() {
                    lo = lo.min(p);
                    hi = hi.max(p);
                    largest = largest.max(s.scale[0].max(s.scale[1]).max(s.scale[2]));
                }
                if degree > 0 {
                    let c = &sh[i * per..(i + 1) * per];
                    for pair in c.chunks(2) {
                        let a = f16_bits(pair[0]);
                        let b = pair.get(1).map_or(0, |&v| f16_bits(v));
                        packed_sh.push(u32::from(a) | (u32::from(b) << 16));
                    }
                }
                pack(s)
            })
            .collect();
        let pad = Vec3::splat(3.0 * largest.min(1e4));
        let bounds = if lo.x.is_finite() {
            (lo - pad, hi + pad)
        } else {
            (Vec3::ZERO, Vec3::ZERO)
        };
        SplatCloud {
            splats,
            sh_degree: degree,
            sh: packed_sh,
            bounds,
        }
    }

    /// Unpacks splat `i` (tests, tools).
    pub fn raw(&self, i: usize) -> RawSplat {
        let p = &self.splats[i];
        let (sx, sy) = halves(p.scale_opacity[0]);
        let (sz, a) = halves(p.scale_opacity[1]);
        let (r, g) = halves(p.color[0]);
        let (b, _) = halves(p.color[1]);
        RawSplat {
            position: p.position,
            scale: [sx, sy, sz],
            rotation: unpack_quat(p.rotation),
            color: [r, g, b],
            opacity: a,
        }
    }
}

fn halves(w: u32) -> (f32, f32) {
    (f16_to_f32(w as u16), f16_to_f32((w >> 16) as u16))
}

fn pair(a: f32, b: f32) -> u32 {
    u32::from(f16_bits(a)) | (u32::from(f16_bits(b)) << 16)
}

/// Packs one Gaussian.
pub fn pack(s: &RawSplat) -> PackedSplat {
    let sc = |v: f32| if v.is_finite() { v.abs() } else { 0.0 };
    PackedSplat {
        position: s.position,
        rotation: pack_quat(s.rotation),
        scale_opacity: [
            pair(sc(s.scale[0]), sc(s.scale[1])),
            pair(sc(s.scale[2]), s.opacity.clamp(0.0, 1.0)),
        ],
        color: [pair(s.color[0], s.color[1]), pair(s.color[2], 0.0)],
    }
}

/// A unit quaternion `[x, y, z, w]` as smallest-three: the largest component's index in bits
/// 30-31, the other three (in order, signed so the largest is positive) in 10 bits each over
/// `[-1/sqrt 2, 1/sqrt 2]`. Worst-case error about 0.001 per component.
pub fn pack_quat(q: [f32; 4]) -> u32 {
    let mut q = q;
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if !n.is_finite() || n <= 1e-12 {
        q = [0.0, 0.0, 0.0, 1.0];
    } else {
        for c in &mut q {
            *c /= n;
        }
    }
    let mut largest = 0;
    for i in 1..4 {
        if q[i].abs() > q[largest].abs() {
            largest = i;
        }
    }
    let sign = if q[largest] < 0.0 { -1.0 } else { 1.0 };
    let mut out = (largest as u32) << 30;
    let mut shift = 20;
    for (i, &c) in q.iter().enumerate() {
        if i == largest {
            continue;
        }
        let v = (c * sign * std::f32::consts::SQRT_2 * 0.5 + 0.5).clamp(0.0, 1.0);
        out |= ((v * 1023.0).round() as u32) << shift;
        shift -= 10;
    }
    out
}

/// The inverse of [`pack_quat`].
pub fn unpack_quat(p: u32) -> [f32; 4] {
    let largest = (p >> 30) as usize;
    let d = |shift: u32| ((p >> shift) & 1023) as f32 / 1023.0 * 2.0 - 1.0;
    let rest = [d(20), d(10), d(0)].map(|v| v * std::f32::consts::FRAC_1_SQRT_2);
    let m = (1.0 - rest.iter().map(|v| v * v).sum::<f32>())
        .max(0.0)
        .sqrt();
    let mut q = [0.0; 4];
    let mut j = 0;
    for (i, c) in q.iter_mut().enumerate() {
        if i == largest {
            *c = m;
        } else {
            *c = rest[j];
            j += 1;
        }
    }
    let n = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    q.map(|v| v / n)
}

/// IEEE half-precision bits of `v`, rounded to nearest even; out-of-range values saturate to the
/// largest finite half (a splat never wants infinity).
pub fn f16_bits(v: f32) -> u16 {
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    if v.is_nan() {
        return sign | 0x7e00;
    }
    let a = x & 0x7fff_ffff;
    if a >= 0x477f_f000 {
        // >= 65520 rounds to infinity in IEEE; saturate.
        return sign | 0x7bff;
    }
    if a < 0x3880_0000 {
        // Subnormal half (or zero): value = mantissa * 2^-24.
        if a < 0x3300_0000 {
            return sign;
        }
        // In units of 2^-24: m * 2^(e - 150) / 2^-24 = m >> (126 - e), with 14 <= 126 - e <= 24.
        let e = (a >> 23) as i32;
        let m = (a & 0x7f_ffff) | 0x80_0000;
        let shift = (126 - e) as u32;
        let half = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let mid = 1 << (shift - 1);
        let round = rem > mid || (rem == mid && half & 1 == 1);
        return sign | (half + u32::from(round)) as u16;
    }
    // Normal: rebias the exponent from 127 to 15, keep 10 mantissa bits.
    let r = a - 0x3800_0000; // (127 - 15) << 23
    let half = r >> 13;
    let rem = r & 0x1fff;
    let round = rem > 0x1000 || (rem == 0x1000 && half & 1 == 1);
    sign | (half + u32::from(round)) as u16
}

pub fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = (h >> 10) & 0x1f;
    let m = f32::from(h & 0x3ff);
    sign * match e {
        0 => m * 2f32.powi(-24),
        31 => {
            if m == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => (1.0 + m / 1024.0) * 2f32.powi(i32::from(e) - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_round_trip() {
        for v in [
            0.0f32,
            1.0,
            -2.5,
            0.333,
            1e-3,
            1e-5,
            6e-8,
            65504.0,
            1e6,
            -1e-7,
            0.5 + 1.0 / 4096.0,
        ] {
            let back = f16_to_f32(f16_bits(v));
            let tol = (v.abs() * 1e-3).max(6e-8);
            let expect = v.clamp(-65504.0, 65504.0);
            assert!((back - expect).abs() <= tol, "{v} -> {back}");
        }
        // Exact values survive exactly.
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(2f32.powi(-24)), 0x0001);
        assert_eq!(f16_bits(2f32.powi(-14)), 0x0400);
    }

    #[test]
    fn quaternions_round_trip() {
        let qs = [
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0, -1.0],
            [0.5, 0.5, 0.5, 0.5],
            [0.1, -0.7, 0.2, 0.67],
            [-0.9, 0.1, 0.3, 0.2],
        ];
        for q in qs {
            let n = (q.iter().map(|v: &f32| v * v).sum::<f32>()).sqrt();
            let q = q.map(|v| v / n);
            let r = unpack_quat(pack_quat(q));
            let dot: f32 = q.iter().zip(r.iter()).map(|(a, b)| a * b).sum();
            assert!(dot.abs() > 0.99999, "{q:?} -> {r:?}");
        }
    }
}
