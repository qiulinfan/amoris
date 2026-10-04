//! The built-in meshes a `Model` can name without an asset: unit-sized, centred on the origin,
//! +y up, counter-clockwise front faces, with normals, texture coordinates and tangents.

use std::f32::consts::{PI, TAU};

use crate::mesh::{MeshData, Vertex};

/// The primitive names, in the order the renderer preloads them.
pub const PRIMITIVES: [&str; 7] = [
    "cube", "sphere", "plane", "cylinder", "capsule", "cone", "torus",
];

/// The primitive named `name`, or `None`.
pub fn primitive(name: &str) -> Option<MeshData> {
    let mut m = match name {
        "cube" => cube(),
        "sphere" => sphere(32, 16),
        "plane" => plane(),
        "cylinder" => cylinder(32),
        "capsule" => capsule(24, 8),
        "cone" => cone(32),
        "torus" => torus(32, 16, 0.35, 0.15),
        _ => return None,
    };
    m.name = name.to_owned();
    m.compute_tangents();
    Some(m)
}

fn v(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Vertex {
    Vertex {
        position,
        normal,
        uv,
        tangent: [1.0, 0.0, 0.0, 1.0],
    }
}

/// A 1 x 1 x 1 cube: 24 vertices, 12 triangles.
pub fn cube() -> MeshData {
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    let mut vs = Vec::with_capacity(24);
    let mut is = Vec::with_capacity(36);
    for (n, u, w) in faces {
        let base = vs.len() as u32;
        for (s, t) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = [
                (n[0] + u[0] * s + w[0] * t) * 0.5,
                (n[1] + u[1] * s + w[1] * t) * 0.5,
                (n[2] + u[2] * s + w[2] * t) * 0.5,
            ];
            vs.push(v(p, n, [(s + 1.0) * 0.5, 1.0 - (t + 1.0) * 0.5]));
        }
        is.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    MeshData::new("cube", vs, is)
}

/// A UV sphere of diameter 1.
pub fn sphere(segments: u32, rings: u32) -> MeshData {
    let mut vs = Vec::new();
    let mut is = Vec::new();
    for r in 0..=rings {
        let phi = PI * r as f32 / rings as f32;
        for s in 0..=segments {
            let theta = TAU * s as f32 / segments as f32;
            let n = [phi.sin() * theta.cos(), phi.cos(), -phi.sin() * theta.sin()];
            vs.push(v(
                [n[0] * 0.5, n[1] * 0.5, n[2] * 0.5],
                n,
                [s as f32 / segments as f32, r as f32 / rings as f32],
            ));
        }
    }
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * row + s;
            let b = a + row;
            if r != 0 {
                is.extend_from_slice(&[a, b, a + 1]);
            }
            if r != rings - 1 {
                is.extend_from_slice(&[a + 1, b, b + 1]);
            }
        }
    }
    MeshData::new("sphere", vs, is)
}

/// A 1 x 1 plane in x-z facing +y.
pub fn plane() -> MeshData {
    let n = [0.0, 1.0, 0.0];
    let vs = vec![
        v([-0.5, 0.0, 0.5], n, [0.0, 1.0]),
        v([0.5, 0.0, 0.5], n, [1.0, 1.0]),
        v([0.5, 0.0, -0.5], n, [1.0, 0.0]),
        v([-0.5, 0.0, -0.5], n, [0.0, 0.0]),
    ];
    MeshData::new("plane", vs, vec![0, 1, 2, 0, 2, 3])
}

fn disc(vs: &mut Vec<Vertex>, is: &mut Vec<u32>, segments: u32, y: f32, radius: f32, up: bool) {
    let n = [0.0, if up { 1.0 } else { -1.0 }, 0.0];
    let c = vs.len() as u32;
    vs.push(v([0.0, y, 0.0], n, [0.5, 0.5]));
    for s in 0..=segments {
        let t = TAU * s as f32 / segments as f32;
        let (x, z) = (t.cos(), -t.sin());
        vs.push(v(
            [x * radius, y, z * radius],
            n,
            [0.5 + x * 0.5, 0.5 + z * 0.5],
        ));
    }
    for s in 0..segments {
        let (a, b) = (c + 1 + s, c + 2 + s);
        if up {
            is.extend_from_slice(&[c, a, b]);
        } else {
            is.extend_from_slice(&[c, b, a]);
        }
    }
}

/// A cylinder of diameter 1 and height 1.
pub fn cylinder(segments: u32) -> MeshData {
    let mut vs = Vec::new();
    let mut is = Vec::new();
    for s in 0..=segments {
        let t = TAU * s as f32 / segments as f32;
        let n = [t.cos(), 0.0, -t.sin()];
        let u = s as f32 / segments as f32;
        vs.push(v([n[0] * 0.5, -0.5, n[2] * 0.5], n, [u, 1.0]));
        vs.push(v([n[0] * 0.5, 0.5, n[2] * 0.5], n, [u, 0.0]));
    }
    for s in 0..segments {
        let a = s * 2;
        is.extend_from_slice(&[a, a + 2, a + 1, a + 1, a + 2, a + 3]);
    }
    disc(&mut vs, &mut is, segments, 0.5, 0.5, true);
    disc(&mut vs, &mut is, segments, -0.5, 0.5, false);
    MeshData::new("cylinder", vs, is)
}

/// A cone of base diameter 1 and height 1, apex up.
pub fn cone(segments: u32) -> MeshData {
    let mut vs = Vec::new();
    let mut is = Vec::new();
    let slope = 0.5f32.atan2(1.0);
    for s in 0..=segments {
        let t = TAU * s as f32 / segments as f32;
        let n = [t.cos() * slope.cos(), slope.sin(), -t.sin() * slope.cos()];
        let u = s as f32 / segments as f32;
        vs.push(v([t.cos() * 0.5, -0.5, -t.sin() * 0.5], n, [u, 1.0]));
        vs.push(v([0.0, 0.5, 0.0], n, [u, 0.0]));
    }
    for s in 0..segments {
        let a = s * 2;
        is.extend_from_slice(&[a, a + 2, a + 1]);
    }
    disc(&mut vs, &mut is, segments, -0.5, 0.5, false);
    MeshData::new("cone", vs, is)
}

/// A capsule of diameter 0.5 and total height 1 (the physics capsule's proportions).
pub fn capsule(segments: u32, rings: u32) -> MeshData {
    let radius = 0.25f32;
    let half = 0.5 - radius;
    let mut vs = Vec::new();
    let mut is = Vec::new();
    // Rings over a sphere split at the equator, the halves pushed apart by the cylinder.
    let total = rings * 2 + 1;
    for r in 0..=total {
        let (phi, off) = if r <= rings {
            (PI * 0.5 * r as f32 / rings as f32, half)
        } else {
            (PI * 0.5 * (r - 1) as f32 / rings as f32, -half)
        };
        for s in 0..=segments {
            let theta = TAU * s as f32 / segments as f32;
            let n = [phi.sin() * theta.cos(), phi.cos(), -phi.sin() * theta.sin()];
            vs.push(v(
                [n[0] * radius, n[1] * radius + off, n[2] * radius],
                n,
                [s as f32 / segments as f32, r as f32 / total as f32],
            ));
        }
    }
    let row = segments + 1;
    for r in 0..total {
        for s in 0..segments {
            let a = r * row + s;
            let b = a + row;
            is.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    MeshData::new("capsule", vs, is)
}

/// A torus in x-z of ring radius `major` and tube radius `minor`.
pub fn torus(segments: u32, sides: u32, major: f32, minor: f32) -> MeshData {
    let mut vs = Vec::new();
    let mut is = Vec::new();
    for s in 0..=segments {
        let t = TAU * s as f32 / segments as f32;
        let (ct, st) = (t.cos(), -t.sin());
        for k in 0..=sides {
            let p = TAU * k as f32 / sides as f32;
            let (cp, sp) = (p.cos(), p.sin());
            let n = [ct * cp, sp, st * cp];
            vs.push(v(
                [ct * (major + minor * cp), minor * sp, st * (major + minor * cp)],
                n,
                [s as f32 / segments as f32, k as f32 / sides as f32],
            ));
        }
    }
    let row = sides + 1;
    for s in 0..segments {
        for k in 0..sides {
            let a = s * row + k;
            let b = a + row;
            is.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    MeshData::new("torus", vs, is)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_primitive_builds_with_valid_indices() {
        for name in PRIMITIVES {
            let m = primitive(name).unwrap();
            assert!(m.triangles() > 0, "{name}");
            assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()), "{name}");
            assert!(m.bounds.radius > 0.4 && m.bounds.radius < 0.9, "{name} {}", m.bounds.radius);
        }
    }

    #[test]
    fn faces_wind_outward() {
        for name in PRIMITIVES {
        let m = primitive(name).unwrap();
        for t in m.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize].position);
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let vn = m.vertices[t[0] as usize].normal;
            let area = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if area > 1e-9 {
                assert!(n[0] * vn[0] + n[1] * vn[1] + n[2] * vn[2] > 0.0, "{name}");
            }
        }
        }
    }
}
