//! Source maps (version 3) read without the transpiler, so a build that runs compiled modules
//! (the web worker) still names TypeScript lines (script-sandbox.md 5.2). A map is data from
//! outside (a compiled set sent to a worker, a replay's embedded set): every sum is checked, and a
//! map that overflows reads as no map rather than panicking.

use serde::Deserialize;

#[derive(Deserialize)]
struct Raw {
    mappings: String,
}

/// One mapping: a generated column and the source line and column it came from, 0-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Segment {
    gen_col: u32,
    src_line: u32,
    src_col: u32,
}

/// A decoded map: per generated line, its segments ascending by column.
#[derive(Clone, Debug, Default)]
pub struct LineMap {
    lines: Vec<Vec<Segment>>,
}

impl LineMap {
    /// Decodes a map's JSON; an unreadable map gives an empty one (positions then stay as the
    /// JavaScript's).
    pub fn parse(json: &str) -> LineMap {
        let Ok(raw) = serde_json::from_str::<Raw>(json) else {
            return LineMap::default();
        };
        decode(&raw.mappings).unwrap_or_default()
    }

    /// The TypeScript (line, column), 1-based, of a 1-based generated position: the last segment
    /// of that line at or before the column, else the line's first.
    pub fn lookup(&self, line: u32, column: u32) -> Option<(u32, u32)> {
        let segs = self
            .lines
            .get(usize::try_from(line.checked_sub(1)?).ok()?)?;
        let col = column.checked_sub(1)?;
        let i = segs.partition_point(|s| s.gen_col <= col);
        let s = if i == 0 { segs.first()? } else { &segs[i - 1] };
        Some((s.src_line + 1, s.src_col + 1))
    }
}

fn base64(c: u8) -> Option<i64> {
    let v = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(i64::from(v))
}

/// The VLQ fields of one segment.
fn fields(seg: &[u8]) -> Option<Vec<i64>> {
    let mut out = Vec::with_capacity(5);
    let mut value: i64 = 0;
    let mut shift = 0u32;
    for &c in seg {
        let digit = base64(c)?;
        value |= (digit & 31).checked_shl(shift)?;
        if digit & 32 != 0 {
            shift += 5;
            // Past 55 bits the next digit would not fit an i64 with its sign bit.
            if shift > 55 {
                return None;
            }
        } else {
            let negative = value & 1 == 1;
            let magnitude = value >> 1;
            out.push(if negative { -magnitude } else { magnitude });
            value = 0;
            shift = 0;
        }
    }
    Some(out)
}

fn decode(mappings: &str) -> Option<LineMap> {
    let mut lines = Vec::new();
    let (mut src_line, mut src_col, mut src_idx, mut name_idx) = (0i64, 0i64, 0i64, 0i64);
    for line in mappings.split(';') {
        let mut gen_col = 0i64;
        let mut segs = Vec::new();
        for seg in line.split(',').filter(|s| !s.is_empty()) {
            let f = fields(seg.as_bytes())?;
            gen_col = gen_col.checked_add(*f.first()?)?;
            if f.len() >= 4 {
                src_idx = src_idx.checked_add(f[1])?;
                src_line = src_line.checked_add(f[2])?;
                src_col = src_col.checked_add(f[3])?;
                if let Some(n) = f.get(4) {
                    name_idx = name_idx.checked_add(*n)?;
                }
                segs.push(Segment {
                    gen_col: u32::try_from(gen_col).ok()?,
                    src_line: u32::try_from(src_line).ok()?,
                    src_col: u32::try_from(src_col).ok()?,
                });
            }
        }
        segs.sort_by_key(|s| s.gen_col);
        lines.push(segs);
    }
    let _ = (src_idx, name_idx);
    Some(LineMap { lines })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_vlq() {
        // "AAAA;AACA,IAAI": line 1 col 0 -> 0:0; line 2 col 0 -> 1:0, col 4 -> 1:4.
        let m = LineMap::parse(
            r#"{"version":3,"sources":["a.ts"],"names":[],"mappings":"AAAA;AACA,IAAI"}"#,
        );
        assert_eq!(m.lookup(1, 1), Some((1, 1)));
        assert_eq!(m.lookup(2, 1), Some((2, 1)));
        assert_eq!(m.lookup(2, 6), Some((2, 5)));
        assert_eq!(m.lookup(3, 1), None);
        assert_eq!(fields(b"gB"), Some(vec![16]));
        assert_eq!(fields(b"D"), Some(vec![-1]));
    }

    #[test]
    fn a_hostile_map_reads_as_none() {
        // Segments of one field, each near 2^59, overflow the column sum after a few.
        let big = "gggggggggggf";
        assert!(fields(big.as_bytes()).is_some_and(|f| f[0] > 1 << 58));
        let line = vec![big; 40].join(",");
        let json = format!(r#"{{"version":3,"sources":["a.ts"],"names":[],"mappings":"{line}"}}"#);
        assert!(decode(&line).is_none());
        assert_eq!(LineMap::parse(&json).lookup(1, 1), None);
        // Four-field segments overflow the line sum the same way, across lines too.
        let four = format!("A{big}{big}A");
        assert!(decode(&vec![four.as_str(); 40].join(";")).is_none());
        // A digit past 55 bits of shift is refused.
        assert_eq!(fields(b"gggggggggggggB"), None);
    }
}
