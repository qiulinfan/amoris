//! The per-tick stream table (docs/spec/rng.md 5.3): the streams drawn from so far in this tick,
//! keyed by their derived seed, so successive draws under one key continue one sequence. Empty at
//! every boundary, where asking for a stream fails with `rng.outside_tick`; a mark and a rollback
//! undo a failed invocation's draws.

use std::collections::BTreeSet;

use bevy_ecs::prelude::Resource;
use pocket_contract::Problem;

use super::streams::StreamMap;
use super::{
    Deriver, KeyPart, Pcg32, WorldSeed, check_part, key_invalid, outside_tick, stream_from_seed,
};
use crate::entity::EntityId;
use crate::time::Tick;

/// Which stream (rng.md 5.2).
#[derive(Clone, Copy, Debug)]
pub enum StreamKind<'a> {
    /// The default draws of a system in this tick, and `Math.random()` in its scripts.
    System(&'a str),
    /// Per-entity rolls that do not change when other entities appear or vanish.
    Entity(&'a str, EntityId),
    /// Several independent purposes inside one system: Integer, String and Entity parts.
    Named(&'a str, &'a [KeyPart<'a>]),
    /// The same numbers whenever and by whichever system they are drawn: one or more Integer,
    /// String and Entity parts, nothing else.
    Timeless(&'a [KeyPart<'a>]),
}

/// A stream's identity within a tick: its derived seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamKey(pub u64);

/// The table. Derived world state (persistence.md 2): rebuilt as the empty table at any boundary,
/// never serialized, hashed or iterated for a result.
#[derive(Resource, Debug, Default)]
pub struct RngTable {
    entries: StreamMap,
    running: Option<(Tick, WorldSeed)>,
    last_tick: Tick,
    journal: Option<Journal>,
}

#[derive(Debug, Default)]
struct Journal {
    touched: BTreeSet<u64>,
    previous: Vec<(u64, Option<Pcg32>)>,
}

/// Taken when an invocation begins; passed back to commit or roll back its draws.
#[derive(Debug)]
#[must_use]
pub struct RngMark(());

impl RngTable {
    /// Opens the table for tick `tick` with the world seed (`sim.begin`).
    pub(crate) fn begin_tick(&mut self, tick: Tick, seed: WorldSeed) {
        self.entries.clear();
        self.journal = None;
        self.running = Some((tick, seed));
    }

    /// Clears the table at the end of the tick (`sim.finish`).
    pub(crate) fn end_tick(&mut self) {
        if let Some((t, _)) = self.running.take() {
            self.last_tick = t;
        }
        self.entries.clear();
        self.journal = None;
    }

    /// No stream and no tick running: the state every boundary has (simulation.md 4.1, invariant 3).
    pub fn is_empty(&self) -> bool {
        self.entries.len() == 0 && self.running.is_none() && self.journal.is_none()
    }

    /// The tick drawing now, if one runs.
    pub fn tick(&self) -> Option<Tick> {
        self.running.map(|r| r.0)
    }

    /// The key of a stream in this tick, its parts checked (`rng.key_invalid`); `rng.outside_tick`
    /// at a boundary.
    pub fn key(&self, kind: StreamKind<'_>) -> Result<StreamKey, Problem> {
        let Some((tick, seed)) = self.running else {
            return Err(outside_tick(self.last_tick));
        };
        let mut d = Deriver::new(seed.0);
        match kind {
            StreamKind::System(k) => {
                d.part(&KeyPart::Tick(tick));
                d.part(&KeyPart::System(k));
            }
            StreamKind::Entity(k, e) => {
                d.part(&KeyPart::Tick(tick));
                d.part(&KeyPart::System(k));
                d.part(&KeyPart::Entity(e));
            }
            StreamKind::Named(k, parts) => {
                check_free_parts(parts, 2)?;
                d.part(&KeyPart::Tick(tick));
                d.part(&KeyPart::System(k));
                for p in parts {
                    d.part(p);
                }
            }
            StreamKind::Timeless(parts) => {
                if parts.is_empty() {
                    return Err(key_invalid("(none)", 0));
                }
                check_free_parts(parts, 0)?;
                for p in parts {
                    d.part(p);
                }
            }
        }
        Ok(StreamKey(d.finish()))
    }

    /// The generator of a stream, created on its first use in this tick. During an invocation
    /// (between [`RngTable::mark`] and its commit or rollback) the entry's state before the
    /// invocation's first draw is kept for a rollback.
    pub fn stream(&mut self, key: StreamKey) -> Result<&mut Pcg32, Problem> {
        if self.running.is_none() {
            return Err(outside_tick(self.last_tick));
        }
        if let Some(j) = &mut self.journal
            && j.touched.insert(key.0)
        {
            j.previous.push((key.0, self.entries.get(key.0)));
        }
        Ok(self
            .entries
            .get_or_insert(key.0, || stream_from_seed(key.0)))
    }

    /// The system stream of `system` (its key in the schedule) in this tick.
    pub fn system(&mut self, system: &str) -> Result<&mut Pcg32, Problem> {
        let k = self.key(StreamKind::System(system))?;
        self.stream(k)
    }

    /// The entity stream of `entity` for `system` in this tick.
    pub fn entity(&mut self, system: &str, entity: EntityId) -> Result<&mut Pcg32, Problem> {
        let k = self.key(StreamKind::Entity(system, entity))?;
        self.stream(k)
    }

    /// A named stream of `system` in this tick.
    pub fn named(&mut self, system: &str, parts: &[KeyPart<'_>]) -> Result<&mut Pcg32, Problem> {
        let k = self.key(StreamKind::Named(system, parts))?;
        self.stream(k)
    }

    /// A timeless stream.
    pub fn timeless(&mut self, parts: &[KeyPart<'_>]) -> Result<&mut Pcg32, Problem> {
        let k = self.key(StreamKind::Timeless(parts))?;
        self.stream(k)
    }

    /// Starts recording the state each stream had before the invocation's first draw from it.
    pub fn mark(&mut self) -> RngMark {
        self.journal = Some(Journal::default());
        RngMark(())
    }

    /// Keeps the invocation's draws.
    pub fn commit(&mut self, _mark: RngMark) {
        self.journal = None;
    }

    /// Undoes the invocation's draws: entries it changed get their earlier state back and entries
    /// it created are removed, so its draws are as if they never happened.
    pub fn rollback(&mut self, _mark: RngMark) {
        if let Some(j) = self.journal.take() {
            for (k, prev) in j.previous.into_iter().rev() {
                match prev {
                    Some(g) => self.entries.insert(k, g),
                    None => self.entries.remove(k),
                }
            }
        }
    }
}

/// Named and timeless keys take Integer, String and Entity parts only, integers within
/// ±(2^53 - 1).
fn check_free_parts(parts: &[KeyPart<'_>], offset: usize) -> Result<(), Problem> {
    for (i, p) in parts.iter().enumerate() {
        if matches!(p, KeyPart::Tick(_) | KeyPart::System(_)) {
            return Err(key_invalid(&format!("{p:?}"), i + offset));
        }
        check_part(p, i + offset)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_a_tick_nothing_draws() {
        let mut t = RngTable::default();
        assert_eq!(t.system("s").unwrap_err().code, "rng.outside_tick");
        t.begin_tick(Tick(1), WorldSeed(5));
        assert!(t.system("s").is_ok());
        assert!(!t.is_empty());
        t.end_tick();
        assert!(t.is_empty());
        let e = t.system("s").unwrap_err();
        assert_eq!(e.detail["tick"], serde_json::json!(1));
    }

    #[test]
    fn same_key_continues_one_sequence() {
        let mut t = RngTable::default();
        t.begin_tick(Tick(600), WorldSeed(42));
        let a = t.system("script:spawner").unwrap().next_u32();
        let b = t.system("script:spawner").unwrap().next_u32();
        assert_eq!([a, b], [0xdd62_dfd3, 0x9646_2238]);
    }

    #[test]
    fn rollback_restores_and_removes() {
        let mut t = RngTable::default();
        t.begin_tick(Tick(3), WorldSeed(1));
        t.system("a").unwrap().next_u32();
        let m = t.mark();
        t.system("a").unwrap().next_u32();
        t.system("b").unwrap().next_u32();
        t.rollback(m);
        let mut fresh = RngTable::default();
        fresh.begin_tick(Tick(3), WorldSeed(1));
        fresh.system("a").unwrap().next_u32();
        // "a" continues as if the invocation never drew; "b" starts afresh.
        assert_eq!(
            t.system("a").unwrap().next_u32(),
            fresh.system("a").unwrap().next_u32()
        );
        assert_eq!(
            t.system("b").unwrap().next_u32(),
            fresh.system("b").unwrap().next_u32()
        );
        let m = t.mark();
        t.system("c").unwrap().next_u32();
        t.commit(m);
        assert_ne!(
            t.system("c").unwrap().next_u32(),
            fresh.system("c").unwrap().next_u32()
        );
    }

    #[test]
    fn key_parts_are_checked() {
        let mut t = RngTable::default();
        t.begin_tick(Tick(1), WorldSeed(0));
        let bad = [KeyPart::Int(1 << 53)];
        assert_eq!(t.timeless(&bad).unwrap_err().code, "rng.key_invalid");
        assert_eq!(t.timeless(&[]).unwrap_err().code, "rng.key_invalid");
        let sys = [KeyPart::System("x")];
        assert_eq!(
            t.named("s", &sys).unwrap_err().detail["index"],
            serde_json::json!(2)
        );
        assert!(t.timeless(&[KeyPart::Int(-((1 << 53) - 1))]).is_ok());
    }
}
