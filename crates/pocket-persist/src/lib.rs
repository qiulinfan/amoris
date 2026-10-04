//! Canonical serialization, the world hash, snapshot, restore, fork and replay
//! (docs/spec/persistence.md, replay.md, versions.md).
//!
//! The world is written as the Pocket Canonical Encoding ([`pce`]) in sections, one per persisted
//! type, whose digests make the world hash a two-level tree ([`hash`]). [`snapshot`], [`restore`]
//! and [`fork`] work on any world whose types are declared in a [`Registry`] through
//! `pocket_sim::RegisterPersisted`; physics and scripts register their state there without
//! depending on this crate. Replays record a start snapshot and every applied write with each
//! tick's hash ([`replay`]); `verify`, `lockstep` and `first_divergence` name the first tick where
//! two runs differ and the sections and fields that differ ([`diff`]). Versions, saves and
//! migrations are [`version`], [`save`] and [`migrate`].

#![deny(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

pub mod diff;
pub mod error;
pub mod format;
pub mod hash;
pub mod lock;
pub mod migrate;
pub mod pce;
mod project;
pub mod registry;
pub mod replay;
pub mod save;
mod sections;
pub mod snapshot;
pub mod vectors;
pub mod version;

pub use diff::{FieldDiff, SectionChange, Side, SnapshotDiff, diff};
pub use format::{Fingerprint, ResolvedFormat, ResolvedVariant};
pub use hash::{SectionDigest, SectionKey, SectionKind, TickHash, WorldHash};
pub use registry::{Class, Registry, RegistryBuilder};
pub use replay::{
    Applied, Divergence, DivergenceKind, LockstepOptions, LockstepReport, RecordOptions,
    RecordedWrite, Recorder, Replay, ReplayMode, ReplayOutcome, ReplayReport, ReplaySource, Source,
    Stepper, TickRef, VerifyOptions, first_divergence, lockstep, seek, verify,
};
pub use replay::{LabelMap, RebaseCause, ReplaySummary};
pub use snapshot::{
    RestoreOptions, RestoreReport, SectionData, Snapshot, SnapshotContext, SnapshotHeader, fork,
    fork_into, restore, section_digests, snapshot, world_hash,
};
pub use version::{EngineVersion, FormatTable};

/// A registry with `pocket-sim`'s own declaration, ready for other crates' declarations.
pub fn sim_registry() -> RegistryBuilder {
    let mut b = RegistryBuilder::new();
    pocket_sim::persisted::declare(&mut b);
    b
}
