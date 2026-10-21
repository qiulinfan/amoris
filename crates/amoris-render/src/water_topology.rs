//! Shared planar refinements for transitions between non-nested water triangulations.
//! Vertices carry barycentric stencils for both endpoint surfaces; no old height is cached.
use std::{collections::BTreeMap, sync::OnceLock};
type Point = [f64; 2];
type Triangle = [u32; 3];
pub(crate) struct Geometry {
    pub indices: Vec<u32>,
    pub vertices: Vec<[u32; 4]>,
    pub extra: u32,
    pub vertex_count: u32,
}
pub(crate) fn shared() -> &'static Geometry {
    static GEOMETRY: OnceLock<Geometry> = OnceLock::new();
    GEOMETRY.get_or_init(build)
}
fn xy(v: u32) -> Point {
    [(v % 17) as f64, (v / 17) as f64]
}
fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}
fn area(p: &[Point]) -> f64 {
    (0..p.len())
        .map(|n| cross(p[n], p[(n + 1) % p.len()]))
        .sum::<f64>()
        * 0.5
}
fn weights(p: Point, t: Triangle) -> Option<[f64; 3]> {
    let [a, b, c] = t.map(xy);
    let det = cross(sub(b, a), sub(c, a));
    let u = cross(sub(p, a), sub(c, a)) / det;
    let v = cross(sub(b, a), sub(p, a)) / det;
    let mut w = [1.0 - u - v, u, v];
    if w.iter().any(|&n| n < -1e-9) {
        return None;
    }
    for n in &mut w {
        if n.abs() < 1e-12 {
            *n = 0.0;
        }
        if (*n - 1.0).abs() < 1e-12 {
            *n = 1.0;
        }
    }
    Some(w)
}
fn canonical(p: Point, tris: &[Triangle]) -> (Triangle, [f64; 3]) {
    for &t in tris {
        if let Some(w) = weights(p, t) {
            return (t, w);
        }
    }
    panic!("transition point outside endpoint domain: {p:?}");
}
fn intersection(a: Triangle, b: Triangle) -> Vec<Point> {
    let a = a.map(xy);
    let b = b.map(xy);
    for axis in 0..2 {
        let amin = a.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min);
        let amax = a.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max);
        let bmin = b.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min);
        let bmax = b.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max);
        if amax <= bmin || bmax <= amin {
            return Vec::new();
        }
    }
    let mut polygon = a.to_vec();
    for edge in 0..3 {
        let start = b[edge];
        let end = b[(edge + 1) % 3];
        let line = sub(end, start);
        let input = std::mem::take(&mut polygon);
        if input.is_empty() {
            break;
        }
        let mut previous = *input.last().unwrap();
        let mut d0 = cross(line, sub(previous, start));
        for current in input {
            let d1 = cross(line, sub(current, start));
            let in0 = d0 <= 1e-12;
            let in1 = d1 <= 1e-12;
            if in0 != in1 {
                let t = d0 / (d0 - d1);
                polygon.push([
                    previous[0] + (current[0] - previous[0]) * t,
                    previous[1] + (current[1] - previous[1]) * t,
                ]);
            }
            if in1 {
                polygon.push(current);
            }
            previous = current;
            d0 = d1;
        }
        polygon.dedup_by(|a, b| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() < 1e-10);
        if polygon.len() > 1
            && (polygon[0][0] - polygon.last().unwrap()[0]).abs()
                + (polygon[0][1] - polygon.last().unwrap()[1]).abs()
                < 1e-10
        {
            polygon.pop();
        }
    }
    if polygon.len() < 3 || area(&polygon).abs() < 1e-10 {
        Vec::new()
    } else {
        polygon
    }
}
pub(crate) fn levels() -> Vec<Vec<Triangle>> {
    let mut levels = vec![Vec::new()];
    for z in 0..16 {
        for x in 0..16 {
            let a = x + 17 * z;
            levels[0].extend([[a, a + 17, a + 18], [a, a + 18, a + 1]]);
        }
    }
    for step in [4, 8, 16] {
        let mut tris = Vec::new();
        for z in (0..16).step_by(step) {
            for x in (0..16).step_by(step) {
                let mut edge = Vec::new();
                for n in 0..step {
                    edge.push((x, z + n));
                }
                for n in 0..step {
                    edge.push((x + n, z + step));
                }
                for n in 0..step {
                    edge.push((x + step, z + step - n));
                }
                for n in 0..step {
                    edge.push((x + step - n, z));
                }
                let centre = (x + step / 2 + 17 * (z + step / 2)) as u32;
                for n in 0..edge.len() {
                    let a = edge[n];
                    let b = edge[(n + 1) % edge.len()];
                    tris.push([centre, (a.0 + 17 * a.1) as u32, (b.0 + 17 * b.1) as u32]);
                }
            }
        }
        levels.push(tris);
    }
    levels
}
// Template ids 4..9 encode (0,1),(0,2),(0,3),(1,2),(1,3),(2,3).
pub(crate) fn pairs() -> [(usize, usize); 6] {
    [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)]
}
fn build() -> Geometry {
    let levels = levels();
    let mut indices = vec![0u32; 20];
    let mut vertices = vec![[0u32; 4]];
    for (level, tris) in levels.iter().enumerate() {
        indices[level * 2] = indices.len() as u32;
        indices[level * 2 + 1] = (tris.len() * 3) as u32;
        indices.extend(tris.iter().flatten());
    }
    for (pair, (a, b)) in pairs().into_iter().enumerate() {
        let template = pair + 4;
        indices[template * 2] = indices.len() as u32;
        let mut points = BTreeMap::<[i64; 2], u32>::new();
        for &ta in &levels[a] {
            for &tb in &levels[b] {
                let polygon = intersection(ta, tb);
                if polygon.is_empty() {
                    continue;
                }
                let mut local = Vec::new();
                for mut p in polygon {
                    for n in &mut p {
                        if (*n - n.round()).abs() < 1e-10 {
                            *n = n.round();
                        }
                    }
                    let key = p.map(|n| (n * 4294967296.0).round() as i64);
                    let id = *points.entry(key).or_insert_with(|| {
                        let (ia, wa) = canonical(p, &levels[a]);
                        let (ib, wb) = canonical(p, &levels[b]);
                        let id = ((vertices.len() - 1) / 4) as u32;
                        vertices.extend([
                            [
                                (p[0] as f32).to_bits(),
                                (p[1] as f32).to_bits(),
                                ia[0],
                                ia[1],
                            ],
                            [
                                ia[2],
                                (wa[0] as f32).to_bits(),
                                (wa[1] as f32).to_bits(),
                                (wa[2] as f32).to_bits(),
                            ],
                            [ib[0], ib[1], ib[2], (wb[0] as f32).to_bits()],
                            [(wb[1] as f32).to_bits(), (wb[2] as f32).to_bits(), 0, 0],
                        ]);
                        id
                    });
                    local.push(id);
                }
                for n in 1..local.len() - 1 {
                    indices.extend([local[0], local[n], local[n + 1]]);
                }
            }
        }
        indices[template * 2 + 1] = indices.len() as u32 - indices[template * 2];
    }
    let vertex_count = ((vertices.len() - 1) / 4) as u32;
    let state_start = vertices.len() as u32;
    let largest = (0..10).map(|n| indices[n * 2 + 1]).max().unwrap();
    let extra = largest.saturating_sub(1536);
    vertices[0] = [vertex_count, state_start, extra, 0];
    Geometry {
        indices,
        vertices,
        extra,
        vertex_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(level: &[Triangle], p: Point, h: &[f64]) -> f64 {
        let (t, w) = canonical(p, level);
        (0..3).map(|n| h[t[n] as usize] * w[n]).sum()
    }
    #[test]
    fn bridges_cover_the_domain_and_match_both_endpoint_surfaces() {
        let g = shared();
        let levels = levels();
        let h = (0..289)
            .map(|n| 1.0 + (n % 17) as f64 * 0.003 + ((n * 47) % 13) as f64 * 0.01)
            .collect::<Vec<_>>();
        eprintln!(
            "bridge vertices={} extra={} prefix bytes={}",
            g.vertex_count,
            g.extra,
            g.vertices.len() * 16
        );
        for (pair, (a, b)) in pairs().into_iter().enumerate() {
            let template = pair + 4;
            let idx = &g.indices[g.indices[template * 2] as usize
                ..(g.indices[template * 2] + g.indices[template * 2 + 1]) as usize];
            let point = |id: u32| {
                let row = g.vertices[1 + id as usize * 4];
                [f32::from_bits(row[0]) as f64, f32::from_bits(row[1]) as f64]
            };
            let mut total = 0.0;
            let mut edges = BTreeMap::new();
            for tri in idx.as_chunks::<3>().0 {
                let p = tri.map(point);
                assert!(area(&p) < 0.0);
                total -= area(&p);
                for n in 0..3 {
                    *edges
                        .entry((tri[n].min(tri[(n + 1) % 3]), tri[n].max(tri[(n + 1) % 3])))
                        .or_insert(0) += 1;
                }
                for id in tri {
                    let r = &g.vertices[1 + *id as usize * 4..1 + (*id as usize + 1) * 4];
                    let p = point(*id);
                    let from = h[r[0][2] as usize] * f32::from_bits(r[1][1]) as f64
                        + h[r[0][3] as usize] * f32::from_bits(r[1][2]) as f64
                        + h[r[1][0] as usize] * f32::from_bits(r[1][3]) as f64;
                    let to = h[r[2][0] as usize] * f32::from_bits(r[2][3]) as f64
                        + h[r[2][1] as usize] * f32::from_bits(r[3][0]) as f64
                        + h[r[2][2] as usize] * f32::from_bits(r[3][1]) as f64;
                    assert!((from - sample(&levels[a], p, &h)).abs() < 1e-6);
                    assert!((to - sample(&levels[b], p, &h)).abs() < 1e-6);
                }
                let centre = [
                    p.iter().map(|p| p[0]).sum::<f64>() / 3.0,
                    p.iter().map(|p| p[1]).sum::<f64>() / 3.0,
                ];
                for level in [a, b] {
                    let interpolated = p
                        .iter()
                        .map(|&p| sample(&levels[level], p, &h))
                        .sum::<f64>()
                        / 3.0;
                    assert!(
                        (interpolated - sample(&levels[level], centre, &h)).abs() < 1e-6,
                        "bridge crosses an endpoint plane"
                    );
                }
            }
            assert!(
                (total - 256.0).abs() < 1e-4,
                "{a}/{b} has gap/overlap: {total}"
            );
            let mut boundary = 0;
            for ((a, b), count) in edges {
                let a = point(a);
                let b = point(b);
                let outer = (a[0] == b[0] && (a[0] == 0.0 || a[0] == 16.0))
                    || (a[1] == b[1] && (a[1] == 0.0 || a[1] == 16.0));
                if outer {
                    assert_eq!(count, 1);
                    assert!(((a[0] - b[0]).abs() + (a[1] - b[1]).abs() - 1.0).abs() < 1e-9);
                    boundary += 1;
                } else {
                    assert_eq!(count, 2, "nonconforming internal bridge edge");
                }
            }
            assert_eq!(boundary, 64);
        }
    }
}
