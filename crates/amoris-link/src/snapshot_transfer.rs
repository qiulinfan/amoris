//! Lossless section deltas between consecutively posted snapshots. The previous posted frame,
//! including unacknowledged frames, is the base; skipped simulation publications never are.
//! Every frame lists every current section in canonical order, so absence means removal. Kept
//! sections reuse verified immutable bytes; changed bytes and the resulting root are checked.
use amoris_contract::{Problem, detail};
use amoris_persist::hash::{digest_prefix, section_digest, valid_section_name};
use amoris_persist::pce::{self, Decoder, PceError, write_uleb};
use amoris_persist::{
    Fingerprint, SectionData, SectionDigest, SectionKey, SectionKind, Snapshot, SnapshotHeader,
    WorldHash,
};
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"AMORSDEL";
const FORMAT: u32 = 1;
fn noncanonical(offset: usize, reason: impl Into<String>) -> PceError {
    PceError::Noncanonical {
        offset,
        reason: reason.into(),
    }
}
fn invalid(message: impl Into<String>) -> Problem {
    Problem::new("link.snapshot_invalid", message.into(), detail([]))
}
fn same(a: &SectionData, b: &SectionData) -> bool {
    a.key == b.key
        && a.version == b.version
        && a.fingerprint == b.fingerprint
        && a.digest == b.digest
}
#[derive(Default)]
pub struct SnapshotSender {
    version: u64,
    previous: Option<Snapshot>,
}
impl SnapshotSender {
    pub fn encode(&mut self, version: u64, snapshot: &Snapshot) -> Result<Vec<u8>, Problem> {
        if self.version.checked_add(1) != Some(version) {
            return Err(invalid("snapshot publication version is not consecutive"));
        }
        if snapshot.sections().windows(2).any(|s| s[0].key >= s[1].key) {
            return Err(invalid("snapshot sections are repeated or out of order"));
        }
        let header = pce::to_bytes(snapshot.header()).map_err(|p| invalid(p.problem(0).message))?;
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT.to_le_bytes());
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&version.to_le_bytes());
        write_uleb(&mut out, header.len() as u64);
        out.extend_from_slice(&header);
        write_uleb(&mut out, snapshot.sections().len() as u64);
        for section in snapshot.sections() {
            let kept = self
                .previous
                .as_ref()
                .and_then(|s| s.section(&section.key))
                .is_some_and(|old| same(old, section));
            out.push(u8::from(kept));
            out.push(section.key.kind.code());
            write_uleb(&mut out, section.key.name.len() as u64);
            out.extend_from_slice(section.key.name.as_bytes());
            out.extend_from_slice(&section.version.to_le_bytes());
            out.extend_from_slice(&section.fingerprint.0);
            out.extend_from_slice(&section.digest.0);
            if !kept {
                write_uleb(&mut out, section.bytes.len() as u64);
                out.extend_from_slice(&section.bytes);
            }
        }
        out.extend_from_slice(&snapshot.world_hash().0);
        self.version = version;
        self.previous = Some(snapshot.clone());
        Ok(out)
    }
}
#[derive(Clone, Default)]
pub struct SnapshotReceiver {
    version: u64,
    previous: Option<Snapshot>,
}
impl SnapshotReceiver {
    /// Validate external metadata before accepting this base. Any framing, digest, root or
    /// metadata error leaves the receiver untouched, so a valid retry still has its base.
    pub fn receive(
        &mut self,
        version: u64,
        bytes: &[u8],
        validate: impl FnOnce(&Snapshot) -> Result<(), Problem>,
    ) -> Result<Snapshot, Problem> {
        let snapshot = self
            .decode(version, bytes)
            .map_err(|p| invalid(p.problem(0).message))?;
        validate(&snapshot)?;
        self.version = version;
        self.previous = Some(snapshot.clone());
        Ok(snapshot)
    }
    fn decode(&self, version: u64, bytes: &[u8]) -> Result<Snapshot, PceError> {
        let mut d = Decoder::new(bytes, true);
        if d.take(8)? != MAGIC || d.value::<u32>()? != FORMAT {
            return Err(noncanonical(0, "snapshot transfer magic or format differs"));
        }
        let base = d.value::<u64>()?;
        let incoming = d.value::<u64>()?;
        if base != self.version
            || incoming != version
            || self.version.checked_add(1) != Some(version)
        {
            return Err(noncanonical(
                12,
                "snapshot transfer needs its exact previous publication",
            ));
        }
        let len = usize::try_from(d.uleb()?).unwrap_or(usize::MAX);
        let header: SnapshotHeader = pce::from_bytes(d.take(len)?, true)?;
        let count = d.uleb()?;
        let mut sections: Vec<SectionData> = Vec::new();
        let mut prefixed = Vec::new();
        for _ in 0..count {
            let at = d.pos();
            let kept = d.value::<u8>()?;
            if kept > 1 {
                return Err(noncanonical(at, "snapshot transfer section tag"));
            }
            let kind = SectionKind::from_code(d.value::<u8>()?)
                .ok_or_else(|| noncanonical(at, "snapshot transfer section kind"))?;
            let name: String = d.value()?;
            if !valid_section_name(&name) {
                return Err(noncanonical(at, "snapshot transfer section name"));
            }
            let key = SectionKey { kind, name };
            if sections.last().is_some_and(|s| s.key >= key) {
                return Err(noncanonical(at, "snapshot transfer section order"));
            }
            let section_version = d.value::<u32>()?;
            let fingerprint = Fingerprint(d.value::<[u8; 16]>()?);
            let digest = SectionDigest(d.value::<[u8; 16]>()?);
            let section = if kept == 1 {
                let previous = self
                    .previous
                    .as_ref()
                    .and_then(|s| s.section(&key))
                    .ok_or_else(|| {
                        noncanonical(at, "snapshot transfer kept section is absent from its base")
                    })?;
                if previous.version != section_version
                    || previous.fingerprint != fingerprint
                    || previous.digest != digest
                {
                    return Err(noncanonical(at, "snapshot transfer kept identity differs"));
                }
                previous.clone()
            } else {
                let len = usize::try_from(d.uleb()?).unwrap_or(usize::MAX);
                let data = d.take(len)?;
                prefixed.clear();
                digest_prefix(&mut prefixed, kind, &key.name, section_version);
                prefixed.extend_from_slice(data);
                if section_digest(&prefixed) != digest {
                    return Err(noncanonical(
                        at,
                        "snapshot transfer changed section digest differs",
                    ));
                }
                SectionData {
                    key,
                    version: section_version,
                    fingerprint,
                    digest,
                    bytes: Arc::from(data),
                }
            };
            sections.push(section);
        }
        let stored = WorldHash(d.value::<[u8; 16]>()?);
        d.finish()?;
        let snapshot = Snapshot::from_parts(header, sections);
        if stored != snapshot.world_hash() {
            return Err(noncanonical(
                bytes.len().saturating_sub(16),
                "snapshot transfer world hash differs",
            ));
        }
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn section(name: &str, bytes: Arc<[u8]>) -> SectionData {
        let kind = SectionKind::Resource;
        let version = 1;
        let mut prefixed = Vec::new();
        digest_prefix(&mut prefixed, kind, name, version);
        prefixed.extend_from_slice(&bytes);
        SectionData {
            key: SectionKey::new(kind, name),
            version,
            fingerprint: Fingerprint([7; 16]),
            digest: section_digest(&prefixed),
            bytes,
        }
    }
    fn snapshot(tick: u64, frozen: Option<Arc<[u8]>>) -> Snapshot {
        let mut sections = Vec::new();
        if let Some(data) = frozen {
            sections.push(section("Frozen", data));
        }
        sections.push(section("Small", Arc::from(tick.to_le_bytes())));
        Snapshot::from_parts(
            SnapshotHeader {
                tick: amoris_sim::Tick(tick),
                writes: 0,
                engine: amoris_persist::EngineVersion::unbuilt(),
                bundle: amoris_sim::ContentHash([0; 32]),
            },
            sections,
        )
    }
    #[test]
    fn section_deltas_preserve_exact_snapshots_share_kept_bytes_and_reject_bad_frames_atomically() {
        let frozen: Arc<[u8]> = vec![42; 1024 * 1024].into();
        let a = snapshot(0, Some(frozen.clone()));
        let b = snapshot(1, Some(frozen));
        let mut sender = SnapshotSender::default();
        let first = sender.encode(1, &a).unwrap();
        let next = sender.encode(2, &b).unwrap();
        assert!(first.len() > 1024 * 1024);
        assert!(next.len() < 1024);
        let mut damaged = first.clone();
        let at = damaged.windows(32).position(|w| w == [42; 32]).unwrap();
        damaged[at] ^= 1; // Keep every supplied digest/root intact; corrupt only the payload.
        let mut fresh = SnapshotReceiver::default();
        assert!(fresh.receive(1, &damaged, |_| Ok(())).is_err());
        assert_eq!(fresh.receive(1, &first, |_| Ok(())).unwrap(), a);
        let mut receiver = SnapshotReceiver::default();
        let read = receiver.receive(1, &first, |_| Ok(())).unwrap();
        assert_eq!(read, a);
        let mut missing = SnapshotReceiver::default();
        assert!(missing.receive(2, &next, |_| Ok(())).is_err());
        let mut bad = next.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(receiver.receive(2, &bad, |_| Ok(())).is_err());
        assert!(
            receiver
                .receive(2, &next, |_| Err(invalid("bad outer metadata")))
                .is_err()
        );
        let second = receiver.receive(2, &next, |_| Ok(())).unwrap();
        assert_eq!(second, b);
        assert!(Arc::ptr_eq(
            &read.sections()[0].bytes,
            &second.sections()[0].bytes
        ));
        assert_eq!(Snapshot::from_bytes(&second.to_bytes()).unwrap(), b);
        let removed = snapshot(2, None);
        let third = sender.encode(3, &removed).unwrap();
        assert_eq!(receiver.receive(3, &third, |_| Ok(())).unwrap(), removed);
        // A schema fingerprint is part of kept identity even though the world root covers
        // section data digests; changing it cannot accidentally reuse stale schema metadata.
        let mut changed = b.sections().to_vec();
        changed[0].fingerprint = Fingerprint([8; 16]);
        let changed = Snapshot::from_parts(b.header().clone(), changed);
        let mut sender = SnapshotSender::default();
        sender.encode(1, &b).unwrap();
        assert!(sender.encode(2, &changed).unwrap().len() > 1024 * 1024);
    }
}
