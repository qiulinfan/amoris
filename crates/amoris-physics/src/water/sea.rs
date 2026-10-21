//! Compact, deterministic inputs for GPU sea boundaries. No field sampling/readback on CPU.
use super::{Edge, WaterRegion};
use crate::{Sea, geom};
use amoris_assets::water_field::{FieldCheckpoint, FieldSea, FieldSeaWave, MAX_SEA_WAVES};
use amoris_contract::{Problem, detail};
use amoris_sim::math;

pub(crate) struct Prepared {
    pub forcing: FieldSea,
    pub speed: f64,
    pub depth: f64,
}
fn invalid(message: &str) -> Problem {
    Problem::new("water.invalid_sea", message, detail([]))
}
pub(crate) fn prepare(
    sea: &Sea,
    region: &WaterRegion,
    checkpoint: &FieldCheckpoint,
    origin: [f64; 2],
    time: f64,
) -> Result<Prepared, Problem> {
    if !sea.level.is_finite() || sea.waves.len() > MAX_SEA_WAVES {
        return Err(invalid(
            "sea level must be finite and a coupled sea supports at most 16 waves",
        ));
    }
    let level = sea.level - checkpoint.datum;
    let [nx, nz] = checkpoint.size;
    let mut hmin = f64::INFINITY;
    let mut hmax = 0.0f64;
    for side in 0..4 {
        if region.edges[side] != Edge::Sea {
            continue;
        }
        let count = if side < 2 { nz } else { nx };
        for n in 0..count {
            let [x, z] = match side {
                0 => [0, n],
                1 => [nx - 1, n],
                2 => [n, 0],
                _ => [n, nz - 1],
            };
            let bed = f64::from(f32::from_bits(checkpoint.columns[(x + nx * z) as usize][0]));
            let h = math::max(level - bed, 0.0);
            hmax = math::max(hmax, h);
            if h > 0.0001 {
                hmin = math::min(hmin, h);
            }
        }
    }
    let mut longest = 0.0f64;
    let mut waves = Vec::new();
    let mut amplitude = 0.0;
    let mut speed = 0.0;
    for w in &sea.waves {
        if [w.length, w.height, w.toward_deg, w.phase]
            .iter()
            .any(|v| !v.is_finite())
        {
            return Err(invalid("sea wave numbers must be finite"));
        }
        if w.length <= 0.0 || w.height == 0.0 {
            continue;
        }
        longest = math::max(longest, w.length);
        let k = math::TAU / w.length;
        let omega = math::sqrt(crate::GRAVITY * k);
        let d = geom::from_bearing(w.toward_deg);
        let incoming = (0..4).any(|side| {
            region.edges[side] == Edge::Sea
                && d[if side < 2 { 0 } else { 2 }] * (if side % 2 == 0 { -1.0 } else { 1.0 }) < 0.0
        });
        if !incoming {
            continue;
        }
        // Conservative depth interval admitting lambda_SW >= 4 cells and >= 20 depths.
        // Exact per-face eligibility is evaluated again by the GPU, using its actual bed.
        let resolved = math::pow(4.0 * region.cell_m * omega / math::TAU, 2.0) / crate::GRAVITY;
        let shallow = math::pow(math::TAU / (20.0 * omega), 2.0) * crate::GRAVITY;
        let low = math::max(hmin, resolved);
        let high = math::min(hmax, shallow);
        if low > high * (1.0 + 1e-5) || !low.is_finite() {
            continue;
        }
        let phase = k * (d[0] * origin[0] + d[2] * origin[1]) - omega * time + w.phase;
        if !phase.is_finite() {
            return Err(invalid("sea phase overflowed"));
        }
        let (s, c) = math::sin_cos(phase);
        let a = 0.5 * w.height;
        waves.push(FieldSeaWave {
            amplitude_m: a,
            wave_vector_rad_m: [k * d[0], k * d[2]],
            angular_frequency_rad_s: omega,
            phase_at_start_rad: math::atan2(s, c),
        });
        amplitude += a.abs();
        speed += a.abs() * math::sqrt(crate::GRAVITY / low);
    }
    if longest > 0.0 && hmax > longest / 20.0 * (1.0 + 1e-6) {
        return Err(Problem::new(
            "water.deep_edge",
            "sea boundary is deeper than one twentieth of its longest analytic wave",
            detail([]),
        ));
    }
    let forcing = FieldSea {
        level_relative_m: level,
        waves,
    };
    forcing
        .validate()
        .map_err(|_| invalid("sea forcing does not fit the GPU descriptor"))?;
    let depth = hmax + amplitude;
    let speed = speed + 2.0 * math::sqrt(crate::GRAVITY * depth);
    if !depth.is_finite() || !speed.is_finite() {
        return Err(invalid("sea forcing bounds overflowed"));
    }
    Ok(Prepared {
        forcing,
        speed,
        depth,
    })
}
