//! Color-grading lookup tables (docs/spec/color-grading.md): a 3D table that maps a display-encoded
//! color (the display transform's output, AgX's sRGB-like encoding) to another, read from a
//! `.cube` file (Adobe's Cube LUT Specification 1.0, as DaVinci Resolve and most grading tools
//! write it) and applied by the renderer's display transform. Presentation only: the simulation
//! never reads one.

/// The least and the greatest table size along each axis.
pub const MIN_SIZE: u32 = 2;
pub const MAX_SIZE: u32 = 65;
/// The largest `.cube` file read: a 65-entry table written with nine decimals a value is about
/// 8.8 MB.
pub const MAX_CUBE_BYTES: usize = 16 * 1024 * 1024;

/// A 3D color table of `size`^3 entries, red changing fastest: entry `r + size * (g + size * b)`
/// is the output at input `(r, g, b) / (size - 1)`.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorLut {
    pub size: u32,
    pub table: Vec<[f32; 3]>,
}

impl ColorLut {
    /// The table of `size` entries an axis whose entry at input `c` is `f(c)`.
    pub fn from_fn(size: u32, f: impl Fn([f32; 3]) -> [f32; 3]) -> Result<ColorLut, String> {
        check_size(size)?;
        let n = size as usize;
        let step = 1.0 / (size - 1) as f32;
        let mut table = Vec::with_capacity(n * n * n);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    table.push(f([r as f32 * step, g as f32 * step, b as f32 * step]));
                }
            }
        }
        ColorLut::new(size, table)
    }

    /// The table that changes nothing.
    pub fn identity(size: u32) -> Result<ColorLut, String> {
        ColorLut::from_fn(size, |c| c)
    }

    /// A table from its entries, checked: `size` in range, `size`^3 finite entries.
    pub fn new(size: u32, table: Vec<[f32; 3]>) -> Result<ColorLut, String> {
        check_size(size)?;
        let want = (size as usize).pow(3);
        if table.len() != want {
            return Err(format!(
                "a {size}-entry color table has {want} entries, not {}",
                table.len()
            ));
        }
        if let Some(i) = table.iter().position(|e| e.iter().any(|v| !v.is_finite())) {
            return Err(format!(
                "color table entry {i} is not finite: {:?}",
                table[i]
            ));
        }
        Ok(ColorLut { size, table })
    }

    /// Reads a `.cube` file: `LUT_3D_SIZE` from 2 to 65, the default domain (`DOMAIN_MIN 0 0 0`,
    /// `DOMAIN_MAX 1 1 1`, `LUT_3D_INPUT_RANGE 0 1`), then `size`^3 lines of three numbers, red
    /// changing fastest. `TITLE`, blank lines and `#` comments are skipped; any other keyword, a 1D
    /// table, another domain or a wrong count is refused with the line it is on.
    pub fn from_cube(bytes: &[u8]) -> Result<ColorLut, String> {
        if bytes.len() > MAX_CUBE_BYTES {
            return Err(format!(
                ".cube file is {} bytes; the largest read is {MAX_CUBE_BYTES}",
                bytes.len()
            ));
        }
        let text =
            std::str::from_utf8(bytes).map_err(|e| format!(".cube file is not UTF-8: {e}"))?;
        let mut size = None;
        let mut table = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            let at = i + 1;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut words = line.split_whitespace();
            let first = words.next().unwrap_or_default();
            if first.starts_with(|c: char| c.is_ascii_alphabetic()) {
                if !table.is_empty() {
                    return Err(format!(
                        "line {at}: keyword {first} after the table's entries"
                    ));
                }
                let rest: Vec<&str> = words.collect();
                let numbers = |n: usize| -> Result<Vec<f32>, String> {
                    if rest.len() != n {
                        return Err(format!("line {at}: {first} takes {n} numbers"));
                    }
                    rest.iter()
                        .map(|w| {
                            w.parse::<f32>()
                                .map_err(|_| format!("line {at}: {w:?} is not a number"))
                        })
                        .collect()
                };
                match first {
                    "TITLE" => {}
                    "LUT_3D_SIZE" => {
                        let n = match rest.as_slice() {
                            [w] => w
                                .parse::<u32>()
                                .map_err(|_| format!("line {at}: {w:?} is not a size"))?,
                            _ => return Err(format!("line {at}: LUT_3D_SIZE takes one size")),
                        };
                        check_size(n).map_err(|e| format!("line {at}: {e}"))?;
                        size = Some(n);
                    }
                    "LUT_1D_SIZE" => {
                        return Err(format!(
                            "line {at}: a 1D table; only 3D tables (LUT_3D_SIZE) are read"
                        ));
                    }
                    "DOMAIN_MIN" => {
                        if numbers(3)? != [0.0; 3] {
                            return Err(format!("line {at}: only DOMAIN_MIN 0 0 0 is read"));
                        }
                    }
                    "DOMAIN_MAX" => {
                        if numbers(3)? != [1.0; 3] {
                            return Err(format!("line {at}: only DOMAIN_MAX 1 1 1 is read"));
                        }
                    }
                    "LUT_3D_INPUT_RANGE" => {
                        if numbers(2)? != [0.0, 1.0] {
                            return Err(format!("line {at}: only LUT_3D_INPUT_RANGE 0 1 is read"));
                        }
                    }
                    other => {
                        return Err(format!(
                            "line {at}: unknown keyword {other} (read: TITLE, LUT_3D_SIZE, \
                             DOMAIN_MIN, DOMAIN_MAX, LUT_3D_INPUT_RANGE)"
                        ));
                    }
                }
                continue;
            }
            let Some(n) = size else {
                return Err(format!("line {at}: an entry before LUT_3D_SIZE"));
            };
            let mut e = [0.0f32; 3];
            let mut k = 0;
            for w in line.split_whitespace() {
                if k == 3 {
                    return Err(format!("line {at}: more than three numbers"));
                }
                e[k] = w
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("line {at}: {w:?} is not a finite number"))?;
                k += 1;
            }
            if k != 3 {
                return Err(format!("line {at}: an entry has three numbers"));
            }
            if table.len() == (n as usize).pow(3) {
                return Err(format!("line {at}: more than {n}^3 entries"));
            }
            table.push(e);
        }
        let size = size.ok_or("no LUT_3D_SIZE line")?;
        ColorLut::new(size, table)
    }

    /// Reads a `.cube` file from disk.
    pub fn load(path: &std::path::Path) -> Result<ColorLut, String> {
        use std::io::Read;
        let file = std::fs::File::open(path)
            .map_err(|e| format!("cannot open color table {}: {e}", path.display()))?;
        let mut bytes = Vec::new();
        file.take(MAX_CUBE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("cannot read color table {}: {e}", path.display()))?;
        ColorLut::from_cube(&bytes)
    }

    /// The table as a `.cube` file.
    pub fn to_cube(&self, title: &str) -> String {
        let mut s = format!("TITLE \"{title}\"\nLUT_3D_SIZE {}\n", self.size);
        for e in &self.table {
            s.push_str(&format!("{} {} {}\n", e[0], e[1], e[2]));
        }
        s
    }

    /// The table at `c`, trilinear between the eight nearest entries, `c` clamped to [0, 1]: the
    /// display transform's `graded` (post.wgsl), on the CPU.
    pub fn sample(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let last = (n - 1) as f32;
        let mut i0 = [0u32; 3];
        let mut f = [0.0f32; 3];
        for k in 0..3 {
            let p = c[k].clamp(0.0, 1.0) * last;
            i0[k] = (p as u32).min(n - 2);
            f[k] = p - i0[k] as f32;
        }
        let at = |dr: u32, dg: u32, db: u32| {
            let i = (i0[0] + dr) + n * ((i0[1] + dg) + n * (i0[2] + db));
            self.table[i as usize]
        };
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
            [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]
        };
        let x00 = lerp(at(0, 0, 0), at(1, 0, 0), f[0]);
        let x10 = lerp(at(0, 1, 0), at(1, 1, 0), f[0]);
        let x01 = lerp(at(0, 0, 1), at(1, 0, 1), f[0]);
        let x11 = lerp(at(0, 1, 1), at(1, 1, 1), f[0]);
        lerp(lerp(x00, x10, f[1]), lerp(x01, x11, f[1]), f[2])
    }
}

fn check_size(size: u32) -> Result<(), String> {
    if (MIN_SIZE..=MAX_SIZE).contains(&size) {
        Ok(())
    } else {
        Err(format!(
            "a color table has {MIN_SIZE} to {MAX_SIZE} entries an axis, not {size}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cube_file_reads_with_red_changing_fastest() {
        let text = "# graded in a test\nTITLE \"swap\"\n\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\n\
                    DOMAIN_MAX 1.0 1.0 1.0\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n\
                    1 1 1\n";
        let lut = ColorLut::from_cube(text.as_bytes()).unwrap();
        assert_eq!(lut, ColorLut::identity(2).unwrap());
        // Trilinear between the entries: a 2-entry identity is the identity everywhere.
        let c = [0.25, 0.5, 0.875];
        assert_eq!(lut.sample(c), c);
    }

    #[test]
    fn a_table_round_trips_through_its_cube_text() {
        let lut = ColorLut::from_fn(5, |[r, g, b]| [b, r * 0.5, g + 0.25]).unwrap();
        let back = ColorLut::from_cube(lut.to_cube("t").as_bytes()).unwrap();
        assert_eq!(back, lut);
        // Inputs on the grid read their entries; outside [0, 1] they are clamped.
        assert_eq!(lut.sample([0.5, 0.25, 1.0]), [1.0, 0.25, 0.5]);
        assert_eq!(lut.sample([2.0, -1.0, 1.0]), [1.0, 0.5, 0.25]);
    }

    #[test]
    fn the_identity_at_power_of_two_plus_one_sizes_reads_back_its_input() {
        for n in [17, 33, 65] {
            let lut = ColorLut::identity(n).unwrap();
            for i in 0..=255u32 {
                let v = i as f32 / 255.0;
                let s = lut.sample([v, 1.0 - v, v * 0.5]);
                for (a, b) in s.iter().zip([v, 1.0 - v, v * 0.5]) {
                    assert!((a - b).abs() < 1e-6, "{n}: {a} for {b}");
                }
            }
        }
    }

    #[test]
    fn malformed_cube_files_are_refused_with_their_line() {
        let refused = |text: &str, want: &str| {
            let e = ColorLut::from_cube(text.as_bytes()).unwrap_err();
            assert!(e.contains(want), "{text:?}: {e}");
        };
        refused("LUT_1D_SIZE 4\n", "1D table");
        refused("LUT_3D_SIZE 1\n", "2 to 65");
        refused("LUT_3D_SIZE 66\n", "2 to 65");
        refused(
            "LUT_3D_SIZE 2\nDOMAIN_MAX 2 2 2\n",
            "line 2: only DOMAIN_MAX 1 1 1",
        );
        refused(
            "LUT_3D_SIZE 2\nLUT_3D_INPUT_RANGE 0 4\n",
            "LUT_3D_INPUT_RANGE 0 1",
        );
        refused("LUT_3D_SIZE 2\nGAMMA 2.2\n", "unknown keyword GAMMA");
        refused("0 0 0\n", "before LUT_3D_SIZE");
        refused(
            "LUT_3D_SIZE 2\n0 0 0\n1 0\n",
            "line 3: an entry has three numbers",
        );
        refused("LUT_3D_SIZE 2\n0 0 0 0\n", "more than three");
        refused("LUT_3D_SIZE 2\n0 0 nan\n", "not a finite number");
        refused("LUT_3D_SIZE 2\n0 0 0\n", "8 entries, not 1");
        refused(
            "LUT_3D_SIZE 2\n0 0 0\nTITLE \"late\"\n",
            "after the table's entries",
        );
        refused("", "no LUT_3D_SIZE");
        let nine = "LUT_3D_SIZE 2\n".to_owned() + &"0 0 0\n".repeat(9);
        refused(&nine, "line 10: more than 2^3 entries");
    }
}
