//! Splat files: the 3D Gaussian Splatting training output (`.ply`, binary little endian) and
//! antimatter15's 32-byte `.splat`, decoded and packed (cloud.rs); a `.ply` writer for generated
//! clouds; and, natively, a worker thread that loads files under a root so a large cloud never
//! freezes a frame.

use super::cloud::{RawSplat, SH_C0, SplatCloud, sh_coeffs};

/// Decodes a file by its extension (`.ply` or `.splat`).
pub fn parse(name: &str, bytes: &[u8]) -> Result<SplatCloud, String> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".splat") {
        parse_splat(bytes)
    } else if lower.ends_with(".ply")
        || bytes.starts_with(b"ply\n")
        || bytes.starts_with(b"ply\r\n")
    {
        parse_ply(bytes)
    } else {
        Err(format!("{name}: not a .ply or .splat file"))
    }
}

/// antimatter15's format: per splat, position `f32x3`, scale `f32x3` (linear), color `u8x4`
/// (`0.5 + SH_C0 * f_dc` and the sigmoid of the opacity, times 255), rotation `u8x4` (`w x y z`,
/// `q * 128 + 128`).
pub fn parse_splat(bytes: &[u8]) -> Result<SplatCloud, String> {
    if !bytes.len().is_multiple_of(32) {
        return Err(format!(
            ".splat: {} bytes is not a multiple of 32",
            bytes.len()
        ));
    }
    let f = |b: &[u8], o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let (records, _) = bytes.as_chunks::<32>();
    let raw: Vec<RawSplat> = records
        .iter()
        .map(|b| {
            let q = |i: usize| (f32::from(b[28 + i]) - 128.0) / 128.0;
            RawSplat {
                position: [f(b, 0), f(b, 4), f(b, 8)],
                scale: [f(b, 12), f(b, 16), f(b, 20)],
                rotation: [q(1), q(2), q(3), q(0)],
                color: [
                    f32::from(b[24]) / 255.0,
                    f32::from(b[25]) / 255.0,
                    f32::from(b[26]) / 255.0,
                ],
                opacity: f32::from(b[27]) / 255.0,
            }
        })
        .collect();
    Ok(SplatCloud::from_raw(&raw, 0, &[]))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Ty {
    fn parse(s: &str) -> Option<Ty> {
        Some(match s {
            "char" | "int8" => Ty::I8,
            "uchar" | "uint8" => Ty::U8,
            "short" | "int16" => Ty::I16,
            "ushort" | "uint16" => Ty::U16,
            "int" | "int32" => Ty::I32,
            "uint" | "uint32" => Ty::U32,
            "float" | "float32" => Ty::F32,
            "double" | "float64" => Ty::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Ty::I8 | Ty::U8 => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::I32 | Ty::U32 | Ty::F32 => 4,
            Ty::F64 => 8,
        }
    }

    fn read(self, b: &[u8]) -> f32 {
        match self {
            Ty::I8 => f32::from(b[0] as i8),
            Ty::U8 => f32::from(b[0]),
            Ty::I16 => f32::from(i16::from_le_bytes([b[0], b[1]])),
            Ty::U16 => f32::from(u16::from_le_bytes([b[0], b[1]])),
            Ty::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
            Ty::U32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
            Ty::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Ty::F64 => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
        }
    }
}

struct Element {
    name: String,
    count: usize,
    /// (name, type, byte offset in the record).
    props: Vec<(String, Ty, usize)>,
    stride: usize,
}

/// The 3DGS training output: a `vertex` element with `x y z`, optionally `nx ny nz`, `f_dc_0..2`,
/// `f_rest_0..` (channel-major: all red coefficients, then green, then blue), `opacity` (logit),
/// `scale_0..2` (log), `rot_0..3` (`w x y z`, not necessarily normalized).
pub fn parse_ply(bytes: &[u8]) -> Result<SplatCloud, String> {
    let end = find(bytes, b"end_header").ok_or_else(|| ".ply: no end_header".to_owned())?;
    let mut body = end + b"end_header".len();
    if bytes.get(body) == Some(&b'\r') {
        body += 1;
    }
    if bytes.get(body) != Some(&b'\n') {
        return Err(".ply: end_header is not followed by a newline".into());
    }
    body += 1;
    let header = std::str::from_utf8(&bytes[..end]).map_err(|_| ".ply: header is not UTF-8")?;
    let mut lines = header.lines().map(str::trim);
    if lines.next() != Some("ply") {
        return Err(".ply: missing magic".into());
    }
    let mut elements: Vec<Element> = Vec::new();
    for line in lines {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["format", fmt, _] => {
                if *fmt != "binary_little_endian" {
                    return Err(format!(
                        ".ply: format {fmt} unsupported (only binary_little_endian)"
                    ));
                }
            }
            ["element", name, count] => elements.push(Element {
                name: (*name).to_owned(),
                count: count
                    .parse()
                    .map_err(|_| format!(".ply: bad count {count}"))?,
                props: Vec::new(),
                stride: 0,
            }),
            ["property", "list", ..] => {
                return Err(".ply: list properties are unsupported".into());
            }
            ["property", ty, name] => {
                let e = elements.last_mut().ok_or(".ply: property before element")?;
                let ty = Ty::parse(ty).ok_or_else(|| format!(".ply: unknown type {ty}"))?;
                e.props.push(((*name).to_owned(), ty, e.stride));
                e.stride += ty.size();
            }
            _ => {}
        }
    }
    let mut start = body;
    let mut vertex = None;
    for e in &elements {
        if e.name == "vertex" {
            vertex = Some((e, start));
            break;
        }
        start += e.count * e.stride;
    }
    let (v, start) = vertex.ok_or(".ply: no vertex element")?;
    let need = start + v.count * v.stride;
    if bytes.len() < need {
        return Err(format!(
            ".ply: {} bytes, the header needs {need}",
            bytes.len()
        ));
    }
    let prop = |name: &str| v.props.iter().find(|p| p.0 == name).map(|p| (p.1, p.2));
    let req = |name: &str| {
        prop(name).ok_or_else(|| format!(".ply: no property {name} (not a 3DGS file?)"))
    };
    let pos = [req("x")?, req("y")?, req("z")?];
    let dc = [req("f_dc_0")?, req("f_dc_1")?, req("f_dc_2")?];
    let opacity = req("opacity")?;
    let scale = [req("scale_0")?, req("scale_1")?, req("scale_2")?];
    let rot = [req("rot_0")?, req("rot_1")?, req("rot_2")?, req("rot_3")?];
    let mut rest = Vec::new();
    while let Some(p) = prop(&format!("f_rest_{}", rest.len())) {
        rest.push(p);
    }
    // 3 x ((d+1)^2 - 1) rest coefficients for degree d.
    let degree = match rest.len() {
        0 => 0,
        9 => 1,
        24 => 2,
        45 => 3,
        n => {
            return Err(format!(
                ".ply: {n} f_rest properties is not degree 1, 2 or 3"
            ));
        }
    };
    let per = sh_coeffs(degree);
    let mut raw = Vec::with_capacity(v.count);
    let mut sh = Vec::with_capacity(v.count * per * 3);
    let sigmoid = |x: f32| 1.0 / (1.0 + (-x).exp());
    for i in 0..v.count {
        let rec = &bytes[start + i * v.stride..start + (i + 1) * v.stride];
        let get = |(ty, off): (Ty, usize)| ty.read(&rec[off..]);
        let r = rot.map(get);
        raw.push(RawSplat {
            position: pos.map(get),
            scale: scale.map(|p| get(p).exp()),
            rotation: [r[1], r[2], r[3], r[0]],
            color: dc.map(|p| 0.5 + SH_C0 * get(p)),
            opacity: sigmoid(get(opacity)),
        });
        // Channel-major in the file, coefficient-major (RGB interleaved) on the GPU.
        for c in 0..per {
            for ch in 0..3 {
                sh.push(get(rest[ch * per + c]));
            }
        }
    }
    Ok(SplatCloud::from_raw(&raw, degree, &sh))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Writes `raw` (and its SH, as [`SplatCloud::from_raw`] takes them) as a 3DGS training `.ply`.
pub fn write_ply(
    raw: &[RawSplat],
    sh_degree: u32,
    sh: &[f32],
    out: &mut impl std::io::Write,
) -> std::io::Result<()> {
    let per = sh_coeffs(sh_degree.min(3));
    let mut h = String::from("ply\nformat binary_little_endian 1.0\n");
    h += &format!("element vertex {}\n", raw.len());
    for p in [
        "x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2",
    ] {
        h += &format!("property float {p}\n");
    }
    for i in 0..per * 3 {
        h += &format!("property float f_rest_{i}\n");
    }
    for p in [
        "opacity", "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
    ] {
        h += &format!("property float {p}\n");
    }
    h += "end_header\n";
    out.write_all(h.as_bytes())?;
    let mut rec: Vec<f32> = Vec::with_capacity(17 + per * 3);
    let mut buf = Vec::with_capacity(raw.len().min(1 << 16) * (17 + per * 3) * 4);
    for (i, s) in raw.iter().enumerate() {
        rec.clear();
        rec.extend_from_slice(&s.position);
        rec.extend_from_slice(&[0.0; 3]);
        rec.extend(s.color.map(|c| (c - 0.5) / SH_C0));
        for ch in 0..3 {
            for c in 0..per {
                rec.push(sh[i * per * 3 + c * 3 + ch]);
            }
        }
        let a = s.opacity.clamp(1e-6, 1.0 - 1e-6);
        rec.push((a / (1.0 - a)).ln());
        rec.extend(s.scale.map(|v| v.max(1e-30).ln()));
        let q = s.rotation;
        rec.extend_from_slice(&[q[3], q[0], q[1], q[2]]);
        for v in &rec {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        if buf.len() > 1 << 22 {
            out.write_all(&buf)?;
            buf.clear();
        }
    }
    out.write_all(&buf)
}

/// Loads splat files under a root on a worker thread.
#[cfg(not(target_arch = "wasm32"))]
pub struct SplatFiles {
    tx: std::sync::mpsc::Sender<String>,
    rx: std::sync::mpsc::Receiver<(String, Result<SplatCloud, String>)>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SplatFiles {
    pub fn new(root: std::path::PathBuf) -> SplatFiles {
        let (tx, jobs) = std::sync::mpsc::channel::<String>();
        let (done, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("pocket-splats".into())
            .spawn(move || {
                for path in jobs {
                    let t = std::time::Instant::now();
                    let r = std::fs::read(root.join(&path))
                        .map_err(|e| format!("{path}: {e}"))
                        .and_then(|b| parse(&path, &b));
                    if let Ok(c) = &r {
                        log::info!(
                            "loaded {path}: {} splats, SH degree {}, in {:.0} ms",
                            c.len(),
                            c.sh_degree,
                            t.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    if done.send((path, r)).is_err() {
                        break;
                    }
                }
            })
            .ok();
        SplatFiles { tx, rx }
    }

    pub fn request(&mut self, path: &str) {
        let _ = self.tx.send(path.to_owned());
    }

    pub fn poll(&mut self) -> Vec<(String, Result<SplatCloud, String>)> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Vec<RawSplat>, Vec<f32>) {
        let raw: Vec<RawSplat> = (0..5)
            .map(|i| {
                let f = i as f32;
                let q = [0.1 * f, -0.2, 0.3, 1.0];
                let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
                RawSplat {
                    position: [f, -f * 0.5, 2.0],
                    scale: [0.01 + 0.01 * f, 0.02, 0.003],
                    rotation: q.map(|v| v / n),
                    color: [0.2 * f, 0.5, 0.9],
                    opacity: 0.1 + 0.2 * f,
                }
            })
            .collect();
        let sh: Vec<f32> = (0..5 * 45)
            .map(|i| ((i * 7) % 13) as f32 * 0.01 - 0.06)
            .collect();
        (raw, sh)
    }

    #[test]
    fn ply_round_trips() {
        let (raw, sh) = sample();
        let mut bytes = Vec::new();
        write_ply(&raw, 3, &sh, &mut bytes).expect("write");
        let cloud = parse("x.ply", &bytes).expect("parse");
        let expect = SplatCloud::from_raw(&raw, 3, &sh);
        assert_eq!(cloud.len(), 5);
        assert_eq!(cloud.sh_degree, 3);
        assert_eq!(cloud.sh.len(), 5 * 23);
        for i in 0..5 {
            let (a, b) = (cloud.raw(i), expect.raw(i));
            for k in 0..3 {
                assert!((a.position[k] - b.position[k]).abs() < 1e-6);
                assert!((a.scale[k] - b.scale[k]).abs() <= b.scale[k] * 2e-3);
                assert!((a.color[k] - b.color[k]).abs() < 2e-3);
            }
            assert!((a.opacity - b.opacity).abs() < 2e-3);
        }
        // SH survive the channel-major file order.
        let diff = cloud
            .sh
            .iter()
            .zip(expect.sh.iter())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(diff, 0);
    }

    #[test]
    fn splat_format_decodes() {
        let mut b = Vec::new();
        for v in [1.0f32, 2.0, 3.0, 0.1, 0.2, 0.3] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[255, 128, 0, 200]);
        // w = 1: (255 - 128) / 128 ~ 0.99.
        b.extend_from_slice(&[255, 128, 128, 128]);
        let c = parse("a.splat", &b).expect("parse");
        let r = c.raw(0);
        assert_eq!(r.position, [1.0, 2.0, 3.0]);
        assert!((r.scale[1] - 0.2).abs() < 1e-3);
        assert!((r.color[0] - 1.0).abs() < 1e-3 && r.color[2] == 0.0);
        assert!((r.opacity - 200.0 / 255.0).abs() < 1e-3);
        assert!(r.rotation[3] > 0.999);
    }

    #[test]
    fn rejects_other_plys() {
        let ply = b"ply\nformat ascii 1.0\nelement vertex 0\nend_header\n";
        assert!(parse("a.ply", ply).is_err());
        let ply = b"ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty float x\nend_header\n\0\0\0\0";
        let e = parse("a.ply", ply).unwrap_err();
        assert!(e.contains("no property y"), "{e}");
    }
}
