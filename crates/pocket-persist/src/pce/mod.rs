//! The Pocket Canonical Encoding, PCE v1 (docs/spec/persistence.md 3): a binary `serde` format
//! whose bytes depend only on the value and its type. Fixed-width little-endian integers, ULEB128
//! lengths, floats bit for bit, maps in iteration order, no field names; decoding accepts exactly
//! what encoding produces.

mod de;
mod ser;

use std::fmt;

use pocket_contract::Problem;
use serde::{Deserialize, Serialize};

pub use de::Decoder;
pub use ser::Encoder;

/// Why encoding or decoding failed. Converted to the `persist` codes with the section and entity
/// it happened in by the callers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PceError {
    /// The bytes end inside a structure (`persist.truncated`).
    Truncated { offset: usize },
    /// Bytes the encoder would not produce, or a value the type refuses (`persist.noncanonical`).
    Noncanonical { offset: usize, reason: String },
    /// A value that cannot be written (`persist.encode`).
    Encode(String),
}

/// An offset not known yet where a nested `Deserialize` raised the error; the decoder fills it in.
const UNKNOWN: usize = usize::MAX;

/// How many compound values (sequences, tuples, maps, structs, newtypes, `Some` and enum variants
/// with content) may enclose a value. The encoder and the decoder count alike and refuse deeper
/// values (`persist.encode`, `persist.noncanonical`), so neither a value a script built nor bytes
/// from a file can recurse until the stack overflows; the JSON conversion and the field diff hold
/// to the same bound (persistence.md 14, choice 21).
pub const MAX_DEPTH: u32 = 128;

impl PceError {
    pub(crate) fn noncanonical(offset: usize, reason: impl Into<String>) -> PceError {
        PceError::Noncanonical {
            offset,
            reason: reason.into(),
        }
    }

    pub(crate) fn at(self, pos: usize) -> PceError {
        match self {
            PceError::Noncanonical { offset, reason } if offset == UNKNOWN => {
                PceError::Noncanonical {
                    offset: pos,
                    reason,
                }
            }
            e => e,
        }
    }

    /// The problem, offsets taken relative to `base` (where the decoded bytes start in a file).
    pub fn problem(&self, base: usize) -> Problem {
        match self {
            PceError::Truncated { offset } => crate::error::truncated(base + offset),
            PceError::Noncanonical { offset, reason } => {
                crate::error::noncanonical(base.saturating_add(*offset), reason)
            }
            PceError::Encode(reason) => crate::error::encode("", None, reason),
        }
    }

    /// The reason in words.
    pub fn reason(&self) -> String {
        match self {
            PceError::Truncated { offset } => format!("the bytes end at offset {offset}"),
            PceError::Noncanonical { reason, .. } | PceError::Encode(reason) => reason.clone(),
        }
    }
}

impl fmt::Display for PceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason())
    }
}

impl std::error::Error for PceError {}

impl serde::ser::Error for PceError {
    fn custom<T: fmt::Display>(msg: T) -> PceError {
        PceError::Encode(msg.to_string())
    }
}

impl serde::de::Error for PceError {
    fn custom<T: fmt::Display>(msg: T) -> PceError {
        PceError::noncanonical(UNKNOWN, msg.to_string())
    }
}

/// Appends the minimal ULEB128 form of `v`.
pub fn write_uleb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        #[allow(clippy::cast_possible_truncation)] // the low seven bits
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Reads a minimal ULEB128 value at `*pos`: at most 10 bytes, no trailing zero byte, within 64 bits.
pub fn read_uleb(bytes: &[u8], pos: &mut usize) -> Result<u64, PceError> {
    let start = *pos;
    let mut v: u64 = 0;
    for i in 0..10u32 {
        let Some(&byte) = bytes.get(*pos) else {
            return Err(PceError::Truncated { offset: *pos });
        };
        *pos += 1;
        let low = u64::from(byte & 0x7f);
        if i == 9 && byte > 1 {
            return Err(PceError::noncanonical(
                start,
                "a ULEB128 value past 64 bits",
            ));
        }
        v |= low << (7 * i);
        if byte & 0x80 == 0 {
            if byte == 0 && i > 0 {
                return Err(PceError::noncanonical(
                    start,
                    "a ULEB128 value with a trailing zero byte",
                ));
            }
            return Ok(v);
        }
    }
    Err(PceError::noncanonical(
        start,
        "a ULEB128 value longer than 10 bytes",
    ))
}

/// Encodes `value` onto `out`. With `finite`, a NaN or an infinity anywhere in it fails (the typed
/// encoders of Component and Resource sections, numeric.md 7); without, floats are written as they
/// are (caches, 3.1).
pub fn encode_into<T: Serialize + ?Sized>(
    value: &T,
    out: &mut Vec<u8>,
    finite: bool,
) -> Result<(), PceError> {
    let mut enc = Encoder::new(out, finite);
    value.serialize(&mut enc)
}

/// Encodes `value`, finite numbers only.
pub fn to_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, PceError> {
    let mut out = Vec::new();
    encode_into(value, &mut out, true)?;
    Ok(out)
}

/// Decodes a whole value from `bytes`; bytes left over are `noncanonical`.
pub fn from_bytes<'de, T: Deserialize<'de>>(bytes: &'de [u8], finite: bool) -> Result<T, PceError> {
    let mut d = Decoder::new(bytes, finite);
    let v = d.value::<T>()?;
    d.finish()?;
    Ok(v)
}

/// Decodes a whole value and refuses bytes that do not encode back to themselves (3.6:
/// `encode(decode(b)) == b`): a map or set whose entries arrive out of their container's order, which
/// the decoder alone cannot see, since PCE writes a map in its iteration order.
pub fn from_bytes_canonical<T>(bytes: &[u8], finite: bool) -> Result<T, PceError>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    let v: T = from_bytes(bytes, finite)?;
    let mut again = Vec::with_capacity(bytes.len());
    encode_into(&v, &mut again, finite)?;
    match again.iter().zip(bytes).position(|(a, b)| a != b) {
        None if again.len() == bytes.len() => Ok(v),
        at => Err(PceError::noncanonical(
            at.unwrap_or(again.len().min(bytes.len())),
            "the value does not encode back to its bytes (entries out of their container's order)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uleb_round_trip_and_rejections() {
        for v in [
            0u64,
            1,
            127,
            128,
            300,
            16_383,
            16_384,
            u64::from(u32::MAX),
            u64::MAX,
        ] {
            let mut out = Vec::new();
            write_uleb(&mut out, v);
            let mut pos = 0;
            assert_eq!(read_uleb(&out, &mut pos).unwrap(), v);
            assert_eq!(pos, out.len());
        }
        let mut pos = 0;
        assert!(matches!(
            read_uleb(&[0x80, 0x00], &mut pos),
            Err(PceError::Noncanonical { .. })
        ));
        let mut pos = 0;
        assert!(matches!(
            read_uleb(&[0x80], &mut pos),
            Err(PceError::Truncated { .. })
        ));
        let mut pos = 0;
        let eleven = [0xffu8; 11];
        assert!(read_uleb(&eleven, &mut pos).is_err());
        let mut pos = 0;
        let past = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
        assert!(read_uleb(&past, &mut pos).is_err());
    }
}
