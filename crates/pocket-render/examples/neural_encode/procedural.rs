//! Procedural PBR materials for the encoder's corpus (no downloads): tileable, built the way an
//! artist's height-driven material is (a height field drives the normal and occlusion, masks drive
//! color, roughness and metalness), so the channels are correlated as real material sets are.

use pocket_assets::neural::Channel;

/// A material's channels at mip 0, texel-major, channels in `channels` order.
pub struct Material {
    pub name: String,
    pub size: u32,
    pub channels: Vec<Channel>,
    pub texels: Vec<f32>,
}

pub const NAMES: [&str; 5] = ["bricks", "metal", "tiles", "wood", "noise"];

fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^ (h >> 15)
}

fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise with period `period` lattice cells over the unit square (tileable).
fn value(u: f32, v: f32, period: i32, seed: u32) -> f32 {
    let x = u * period as f32;
    let y = v * period as f32;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (smooth(x - x0), smooth(y - y0));
    let (ix, iy) = (x0 as i32, y0 as i32);
    let at = |dx: i32, dy: i32| {
        unit(hash(
            (ix + dx).rem_euclid(period),
            (iy + dy).rem_euclid(period),
            seed,
        ))
    };
    let a = at(0, 0) + (at(1, 0) - at(0, 0)) * fx;
    let b = at(0, 1) + (at(1, 1) - at(0, 1)) * fx;
    a + (b - a) * fy
}

/// Fractal value noise in [0, 1].
fn fbm(u: f32, v: f32, period: i32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut p) = (0.0, 0.5, 0.0, period);
    for o in 0..octaves {
        sum += amp * value(u, v, p, seed.wrapping_add(o * 977));
        norm += amp;
        amp *= 0.5;
        p *= 2;
    }
    sum / norm
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    smooth(((x - e0) / (e1 - e0)).clamp(0.0, 1.0))
}

/// Per-texel surface before the normal and occlusion are derived from the height.
struct Surface {
    base: [f32; 3],
    height: f32,
    roughness: f32,
    metallic: f32,
    emissive: [f32; 3],
}

fn bricks(u: f32, v: f32) -> Surface {
    let (cols, rows) = (8.0, 16.0);
    let row = (v * rows).floor();
    let offset = if row as i32 % 2 == 1 { 0.5 } else { 0.0 };
    let bx = u * cols + offset;
    let col = bx.floor();
    let (fx, fy) = (bx - col, v * rows - row);
    let id = hash(col as i32 % cols as i32, row as i32, 7);
    let mortar = 0.06;
    let edge = (fx.min(1.0 - fx) * 2.0).min(fy.min(1.0 - fy)) / mortar;
    let brick = smoothstep(0.6, 1.3, edge);
    let grime = fbm(u, v, 8, 6, 3);
    let tone = unit(id);
    let brick_color = mix([0.55, 0.22, 0.14], [0.72, 0.38, 0.22], tone);
    let brick_color = mix(brick_color, [0.35, 0.3, 0.27], grime * 0.5);
    let mortar_color = [0.62, 0.6, 0.55];
    let pits = fbm(u, v, 64, 3, 11);
    Surface {
        base: mix(mortar_color, brick_color, brick).map(|c| c * (0.85 + 0.3 * pits)),
        height: brick * (0.8 + 0.2 * fbm(u, v, 32, 4, 5)) - 0.1 * pits,
        roughness: 0.95 - brick * (0.15 + 0.2 * pits),
        metallic: 0.0,
        emissive: [0.0; 3],
    }
}

fn metal(u: f32, v: f32) -> Surface {
    let panels = 4.0;
    let (px, py) = (u * panels, v * panels);
    let (fx, fy) = (px - px.floor(), py - py.floor());
    let seam = smoothstep(0.0, 0.02, fx.min(1.0 - fx).min(fy.min(1.0 - fy)));
    // Rivets every eighth of a panel along its edges.
    let rx = (fx * 8.0).fract() - 0.5;
    let ry = (fy * 8.0).fract() - 0.5;
    let near_edge = fx.min(1.0 - fx) < 0.05 || fy.min(1.0 - fy) < 0.05;
    let r2 = (rx * rx + ry * ry) * 16.0;
    let rivet = if near_edge && r2 < 1.0 {
        (1.0 - r2).sqrt()
    } else {
        0.0
    };
    let rust_mask = smoothstep(0.55, 0.65, fbm(u, v, 4, 6, 21));
    let scratches = value(u * 0.05, v * 6.0, 64, 33);
    let steel = [0.62, 0.63, 0.65].map(|c: f32| c * (0.9 + 0.15 * scratches));
    let rust = mix([0.38, 0.17, 0.07], [0.55, 0.3, 0.12], fbm(u, v, 32, 4, 41));
    Surface {
        base: mix(steel, rust, rust_mask),
        height: 0.5 * seam + 0.4 * rivet + 0.1 * rust_mask * fbm(u, v, 64, 3, 51),
        roughness: 0.25 + 0.2 * scratches + rust_mask * 0.5,
        metallic: 1.0 - rust_mask,
        emissive: [0.0; 3],
    }
}

fn tiles(u: f32, v: f32) -> Surface {
    let n = 8.0;
    let (tx, ty) = (u * n, v * n);
    let (ix, iy) = (tx.floor() as i32, ty.floor() as i32);
    let (fx, fy) = (tx - tx.floor(), ty - ty.floor());
    let grout = 0.04;
    let edge = fx.min(1.0 - fx).min(fy.min(1.0 - fy));
    let tile = smoothstep(grout, grout + 0.03, edge);
    let id = hash(ix, iy, 99);
    let palette = [
        [0.85, 0.82, 0.75],
        [0.18, 0.32, 0.45],
        [0.75, 0.55, 0.2],
        [0.2, 0.2, 0.22],
    ];
    let color = palette[(id % 4) as usize];
    let glaze = fbm(u, v, 16, 4, 61);
    // Glowing inlays: rings in some tiles, light in the grout lines of others.
    let (cx, cy) = (fx - 0.5, fy - 0.5);
    let ring = ((cx * cx + cy * cy).sqrt() - 0.28).abs();
    let lit = id.is_multiple_of(3);
    let glow = if lit {
        smoothstep(0.03, 0.0, ring)
    } else {
        0.0
    };
    let glow_color = if id.is_multiple_of(2) {
        [0.1, 0.9, 1.0]
    } else {
        [1.0, 0.55, 0.1]
    };
    Surface {
        base: mix([0.5, 0.48, 0.45], color, tile).map(|c| c * (0.9 + 0.2 * glaze)),
        height: tile * (0.9 + 0.1 * glaze) - 0.15 * glow,
        roughness: 0.9 - tile * 0.75 + 0.1 * glaze,
        metallic: 0.0,
        emissive: glow_color.map(|c| c * glow),
    }
}

fn wood(u: f32, v: f32) -> Surface {
    let planks = 6.0;
    let p = (u * planks).floor();
    let fx = u * planks - p;
    let id = hash(p as i32, 0, 5);
    let offset = unit(id) * 10.0;
    let warp = fbm(u, v, 4, 5, 71 + id % 7);
    let grain =
        ((v * 40.0 + offset + warp * 6.0 + fx * 2.0) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
    let fine = fbm(u, v * 0.25, 128, 3, 81);
    let knot_x = 0.5 + 0.3 * (unit(id >> 3) - 0.5);
    let knot_y = unit(id >> 7);
    let dy = (v - knot_y + 0.5).rem_euclid(1.0) - 0.5;
    let knot = smoothstep(0.08, 0.0, ((fx - knot_x).powi(2) * 0.2 + dy * dy).sqrt());
    let seam = smoothstep(0.0, 0.015, fx.min(1.0 - fx));
    let light = [0.72, 0.52, 0.32];
    let dark = [0.42, 0.25, 0.13];
    let base = mix(light, dark, clamp01(grain * 0.6 + knot * 0.8 + fine * 0.3));
    Surface {
        base: base.map(|c| c * (0.85 + 0.15 * unit(id >> 11)) * (0.6 + 0.4 * seam)),
        height: seam * (0.8 + 0.1 * grain - 0.2 * knot) + 0.05 * fine,
        roughness: 0.55 + 0.25 * grain + 0.1 * fine,
        metallic: 0.0,
        emissive: [0.0; 3],
    }
}

fn noise(u: f32, v: f32) -> Surface {
    let n = |seed: u32| fbm(u, v, 32, 5, seed);
    Surface {
        base: [n(1), n(2), n(3)],
        height: n(4),
        roughness: n(5),
        metallic: smoothstep(0.45, 0.55, n(6)),
        emissive: [n(7) * n(8), n(8) * 0.5, n(9) * 0.2],
    }
}

/// A material by name at `size`x`size` (a power of two).
pub fn material(name: &str, size: u32) -> Option<Material> {
    let (f, channels): (fn(f32, f32) -> Surface, Vec<Channel>) = match name {
        "bricks" => (bricks, nine()),
        "metal" => (metal, eight()),
        "tiles" => (tiles, Channel::ALL.to_vec()),
        "wood" => (wood, nine()),
        "noise" => (noise, Channel::ALL.to_vec()),
        _ => return None,
    };
    let n = size as usize;
    let mut surf = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            surf.push(f(
                (x as f32 + 0.5) / size as f32,
                (y as f32 + 0.5) / size as f32,
            ));
        }
    }
    // Normal from the height (central differences, wrapping), occlusion from the cavity.
    let h = |x: isize, y: isize| {
        surf[(y.rem_euclid(n as isize) as usize) * n + x.rem_euclid(n as isize) as usize].height
    };
    let strength = 4.0 * size as f32 / 1024.0;
    let blur_r = (size / 128).max(1) as isize;
    let mut texels = Vec::with_capacity(n * n * channels.len());
    for y in 0..n as isize {
        for x in 0..n as isize {
            let s = &surf[y as usize * n + x as usize];
            let dx = (h(x + 1, y) - h(x - 1, y)) * strength;
            let dy = (h(x, y + 1) - h(x, y - 1)) * strength;
            let len = (dx * dx + dy * dy + 1.0).sqrt();
            let normal = [-dx / len, -dy / len];
            let mut around = 0.0;
            for (ox, oy) in [
                (-1, 0),
                (1, 0),
                (0, -1),
                (0, 1),
                (-1, -1),
                (1, 1),
                (-1, 1),
                (1, -1),
            ] {
                around += h(x + ox * blur_r, y + oy * blur_r);
            }
            let cavity = (around / 8.0 - s.height).max(0.0);
            let occlusion = clamp01(1.0 - cavity * 2.5);
            for c in &channels {
                texels.push(clamp01(match c {
                    Channel::BaseR => s.base[0],
                    Channel::BaseG => s.base[1],
                    Channel::BaseB => s.base[2],
                    Channel::NormalX => normal[0] * 0.5 + 0.5,
                    Channel::NormalY => normal[1] * 0.5 + 0.5,
                    Channel::Occlusion => occlusion,
                    Channel::Roughness => s.roughness,
                    Channel::Metallic => s.metallic,
                    Channel::EmissiveR => s.emissive[0],
                    Channel::EmissiveG => s.emissive[1],
                    Channel::EmissiveB => s.emissive[2],
                    Channel::Height => s.height,
                }));
            }
        }
    }
    // Quantize to 8 bits, as a stored texture would be.
    for t in &mut texels {
        *t = (*t * 255.0).round() / 255.0;
    }
    Some(Material {
        name: name.to_owned(),
        size,
        channels,
        texels,
    })
}

/// Base color, normal, ORM: eight channels.
fn eight() -> Vec<Channel> {
    vec![
        Channel::BaseR,
        Channel::BaseG,
        Channel::BaseB,
        Channel::NormalX,
        Channel::NormalY,
        Channel::Occlusion,
        Channel::Roughness,
        Channel::Metallic,
    ]
}

/// Base color, normal, ORM and height: nine channels.
fn nine() -> Vec<Channel> {
    let mut c = eight();
    c.push(Channel::Height);
    c
}

/// A photograph (or render) as a material, the way photo-to-material tools derive one: the image
/// is the base color, its luminance the height, roughness falls with brightness.
pub fn from_image(path: &str, size: u32) -> Result<Material, String> {
    let img = image::open(path)
        .map_err(|e| format!("{path}: {e}"))?
        .to_rgb8();
    let (w, h) = img.dimensions();
    let side = w.min(h);
    let crop =
        image::imageops::crop_imm(&img, (w - side) / 2, (h - side) / 2, side, side).to_image();
    let img = image::imageops::resize(&crop, size, size, image::imageops::FilterType::Triangle);
    let n = size as usize;
    let lum: Vec<f32> = img
        .pixels()
        .map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0)
        .collect();
    let channels = nine();
    let l = |x: isize, y: isize| {
        lum[(y.rem_euclid(n as isize) as usize) * n + x.rem_euclid(n as isize) as usize]
    };
    let mut texels = Vec::with_capacity(n * n * channels.len());
    for y in 0..n as isize {
        for x in 0..n as isize {
            let p = img.get_pixel(x as u32, y as u32);
            let dx = (l(x + 1, y) - l(x - 1, y)) * 2.0;
            let dy = (l(x, y + 1) - l(x, y - 1)) * 2.0;
            let len = (dx * dx + dy * dy + 1.0).sqrt();
            let around = (l(x - 2, y) + l(x + 2, y) + l(x, y - 2) + l(x, y + 2)) / 4.0;
            let h = l(x, y);
            for c in &channels {
                texels.push(clamp01(match c {
                    Channel::BaseR => p[0] as f32 / 255.0,
                    Channel::BaseG => p[1] as f32 / 255.0,
                    Channel::BaseB => p[2] as f32 / 255.0,
                    Channel::NormalX => -dx / len * 0.5 + 0.5,
                    Channel::NormalY => -dy / len * 0.5 + 0.5,
                    Channel::Occlusion => 1.0 - (around - h).max(0.0) * 3.0,
                    Channel::Roughness => 0.9 - 0.5 * h,
                    Channel::Metallic => 0.0,
                    Channel::Height => h,
                    _ => 0.0,
                }));
            }
        }
    }
    for t in &mut texels {
        *t = (*t * 255.0).round() / 255.0;
    }
    let name = std::path::Path::new(path)
        .file_stem()
        .map_or("image".into(), |s| s.to_string_lossy().into_owned());
    Ok(Material {
        name,
        size,
        channels,
        texels,
    })
}
