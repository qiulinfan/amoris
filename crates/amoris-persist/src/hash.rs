//! The world hash (docs/spec/persistence.md 5): XXH3-128 per section over its kind, name, version
//! and bytes, then XXH3-128 over the format, the section count and the section digests. Section
//! keys and their canonical order (4.1).

use std::fmt;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

use crate::pce;

/// The seed of section digests: "P3DSEC" and 1.
pub const SECTION_SEED: u64 = 0x5033_4453_4543_0001;
/// The seed of world hashes: "P3DWOR" and 1.
pub const WORLD_SEED: u64 = 0x5033_4457_4f52_0001;
/// The snapshot layout's format number (versions.md 3.4); the world hash covers it.
pub const SNAPSHOT_FORMAT: u32 = 1;

fn hex16(b: &[u8; 16], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for x in b {
        write!(f, "{x:02x}")?;
    }
    Ok(())
}

/// A world hash: XXH3-128's result as its 16 little-endian bytes, shown as 32 hex digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorldHash(pub [u8; 16]);

/// The world hash after a tick (persistence.md 5.3).
pub type TickHash = WorldHash;

/// One section's digest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SectionDigest(pub [u8; 16]);

impl fmt::Display for WorldHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex16(&self.0, f)
    }
}

impl fmt::Debug for WorldHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorldHash(")?;
        hex16(&self.0, f)?;
        f.write_str(")")
    }
}

impl fmt::Display for SectionDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex16(&self.0, f)
    }
}

impl fmt::Debug for SectionDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SectionDigest(")?;
        hex16(&self.0, f)?;
        f.write_str(")")
    }
}

/// A section's kind; its code orders sections (4.1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(u8)]
pub enum SectionKind {
    Entities = 0,
    Resource = 1,
    Component = 2,
    Cache = 3,
}

impl SectionKind {
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<SectionKind> {
        match code {
            0 => Some(SectionKind::Entities),
            1 => Some(SectionKind::Resource),
            2 => Some(SectionKind::Component),
            3 => Some(SectionKind::Cache),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SectionKind::Entities => "entities",
            SectionKind::Resource => "resource",
            SectionKind::Component => "component",
            SectionKind::Cache => "cache",
        }
    }
}

// The kind travels as its one-byte code (the framing's `kind:u8`).
impl Serialize for SectionKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(self.code())
    }
}

impl<'de> Deserialize<'de> for SectionKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<SectionKind, D::Error> {
        let code = u8::deserialize(d)?;
        SectionKind::from_code(code)
            .ok_or_else(|| de::Error::custom(format!("unknown section kind {code}")))
    }
}

/// A section's identity, ordered by kind, then name bytes.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct SectionKey {
    pub kind: SectionKind,
    pub name: String,
}

impl SectionKey {
    pub fn new(kind: SectionKind, name: &str) -> SectionKey {
        SectionKey {
            kind,
            name: name.to_owned(),
        }
    }

    /// The entities section (always named `entities`).
    pub fn entities() -> SectionKey {
        SectionKey::new(SectionKind::Entities, "entities")
    }
}

impl fmt::Display for SectionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.label(), self.name)
    }
}

/// `[A-Za-z][A-Za-z0-9_:.]{0,63}` (4.1).
pub fn valid_section_name(name: &str) -> bool {
    let mut b = name.bytes();
    name.len() <= 64
        && matches!(b.next(), Some(c) if c.is_ascii_alphabetic())
        && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'.'))
}

/// Writes the digest input's prefix, `kind:u8 || PCE(name) || version:u32-LE`, into `out`; the
/// section's data follows it in the same buffer, so one call hashes it (5.4).
pub fn digest_prefix(out: &mut Vec<u8>, kind: SectionKind, name: &str, version: u32) {
    out.push(kind.code());
    pce::write_uleb(out, name.len() as u64);
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&version.to_le_bytes());
}

/// XXH3-128 with `seed` as 16 little-endian bytes.
pub fn xxh3_128(seed: u64, bytes: &[u8]) -> [u8; 16] {
    xxhash_rust::xxh3::xxh3_128_with_seed(bytes, seed).to_le_bytes()
}

/// A section's digest from its prefix and data written by [`digest_prefix`] then the data.
pub fn section_digest(prefixed: &[u8]) -> SectionDigest {
    SectionDigest(xxh3_128(SECTION_SEED, prefixed))
}

/// The world hash of section digests in section order.
pub fn world_hash_of<'a>(digests: impl ExactSizeIterator<Item = &'a SectionDigest>) -> WorldHash {
    let mut buf = Vec::with_capacity(16 + 16 * digests.len());
    buf.extend_from_slice(&SNAPSHOT_FORMAT.to_le_bytes());
    pce::write_uleb(&mut buf, digests.len() as u64);
    for d in digests {
        buf.extend_from_slice(&d.0);
    }
    WorldHash(xxh3_128(WORLD_SEED, &buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_order() {
        assert!(valid_section_name("SimClock"));
        assert!(valid_section_name("rapier3d:PhysicsWorld.v1"));
        assert!(!valid_section_name("1abc"));
        assert!(!valid_section_name("a b"));
        assert!(!valid_section_name(&"a".repeat(65)));
        let mut keys = [
            SectionKey::new(SectionKind::Cache, "A"),
            SectionKey::new(SectionKind::Component, "b"),
            SectionKey::new(SectionKind::Component, "B"),
            SectionKey::entities(),
        ];
        keys.sort();
        let names: Vec<String> = keys.iter().map(ToString::to_string).collect();
        assert_eq!(
            names,
            ["entities:entities", "component:B", "component:b", "cache:A"]
        );
    }
}
