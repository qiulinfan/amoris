//! The error codes of the `persist`, `replay`, `version` and `migrate` families
//! (docs/spec/persistence.md 11, replay.md 4, versions.md 9), one constructor per code, so no call
//! site writes a message of its own.

use pocket_contract::{Detail, Problem, detail};
use serde_json::{Value, json};

use crate::hash::{SectionKey, WorldHash};

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `persist.format {expected, found}`: bad magic, unsupported layout or an unknown section kind.
pub fn format(expected: &str, found: &str) -> Problem {
    Problem::new(
        "persist.format",
        format!("Not a snapshot this engine reads: expected {expected}, found {found}."),
        detail([("expected", json!(expected)), ("found", json!(found))]),
    )
}

/// `persist.truncated {offset}`.
pub fn truncated(offset: usize) -> Problem {
    Problem::new(
        "persist.truncated",
        format!("The bytes end inside a structure at offset {offset}."),
        detail([("offset", json!(offset))]),
    )
}

/// `persist.noncanonical {offset, reason}`.
pub fn noncanonical(offset: usize, reason: &str) -> Problem {
    Problem::new(
        "persist.noncanonical",
        format!("The bytes at offset {offset} are not canonical: {reason}."),
        detail([("offset", json!(offset)), ("reason", json!(reason))]),
    )
}

/// `persist.hash_mismatch {expected, actual}`.
pub fn hash_mismatch(expected: WorldHash, actual: WorldHash) -> Problem {
    Problem::new(
        "persist.hash_mismatch",
        format!("The snapshot's trailing hash is {expected} but its sections hash to {actual}."),
        detail([
            ("expected", json!(expected.to_string())),
            ("actual", json!(actual.to_string())),
        ]),
    )
}

/// `persist.unclassified {type_name}`.
pub fn unclassified(type_name: &str) -> Problem {
    Problem::new(
        "persist.unclassified",
        format!(
            "The world holds {type_name}, which no crate declared as persisted, cache, derived or \
             ignored; register it in the persistence registry."
        ),
        detail([("type_name", json!(type_name))]),
    )
}

/// `persist.type {name, reason}`.
pub fn bad_type(name: &str, reason: &str) -> Problem {
    Problem::new(
        "persist.type",
        format!("The type {name} cannot be persisted: {reason}."),
        detail([("name", json!(name)), ("reason", json!(reason))]),
    )
}

/// `persist.duplicate_name {name}`.
pub fn duplicate_name(name: &str) -> Problem {
    Problem::new(
        "persist.duplicate_name",
        format!("Two types are registered under the section name {name}."),
        detail([("name", json!(name))]),
    )
}

/// `persist.encode {section, entity, reason}`.
pub fn encode(section: &str, entity: Option<u64>, reason: &str) -> Problem {
    let at = entity.map_or(String::new(), |e| format!(" of entity {e}"));
    Problem::new(
        "persist.encode",
        format!("Section {section}{at} cannot be written: {reason}."),
        detail([
            ("section", json!(section)),
            ("entity", json!(entity)),
            ("reason", json!(reason)),
        ]),
    )
}

/// `persist.unknown_section {section}`.
pub fn unknown_section(key: &SectionKey) -> Problem {
    Problem::new(
        "persist.unknown_section",
        format!("The snapshot holds section {key}, which this registry does not know."),
        detail([("section", json!(key.to_string()))]),
    )
}

/// `persist.version {section, snapshot, engine}`: each a `{version, fingerprint}` pair.
pub fn version(key: &SectionKey, snapshot: (u32, &[u8]), engine: (u32, &[u8])) -> Problem {
    let pair = |(v, f): (u32, &[u8])| json!({"version": v, "fingerprint": hex(f)});
    Problem::new(
        "persist.version",
        format!(
            "Section {key} was written at version {} and this engine has version {} (or another \
             shape); load it as a save to migrate it.",
            snapshot.0, engine.0
        ),
        detail([
            ("section", json!(key.to_string())),
            ("snapshot", pair(snapshot)),
            ("engine", pair(engine)),
        ]),
    )
}

/// `persist.orphan {section, entity}`.
pub fn orphan(key: &SectionKey, entity: u64) -> Problem {
    Problem::new(
        "persist.orphan",
        format!("Section {key} has a row for entity {entity}, which is not a live entity."),
        detail([
            ("section", json!(key.to_string())),
            ("entity", json!(entity)),
        ]),
    )
}

/// `persist.not_at_boundary {tick}`.
pub fn not_at_boundary(tick: u64) -> Problem {
    Problem::new(
        "persist.not_at_boundary",
        format!("Tick {tick} is running; snapshots, hashes and restores happen between ticks."),
        detail([("tick", json!(tick))]),
    )
}

/// `persist.restore_mismatch {expected, actual, sections}`.
pub fn restore_mismatch(
    expected: WorldHash,
    actual: WorldHash,
    sections: &[SectionKey],
) -> Problem {
    let names: Vec<String> = sections.iter().map(ToString::to_string).collect();
    Problem::new(
        "persist.restore_mismatch",
        format!(
            "The restored world hashes to {actual}, not the snapshot's {expected} (sections {}): \
             an engine bug.",
            names.join(", ")
        ),
        detail([
            ("expected", json!(expected.to_string())),
            ("actual", json!(actual.to_string())),
            ("sections", json!(names)),
        ]),
    )
}

/// `replay.format {offset, tag}`.
pub fn replay_format(offset: usize, tag: Option<u8>, why: &str) -> Problem {
    Problem::new(
        "replay.format",
        format!("Not a replay this engine reads, at offset {offset}: {why}."),
        detail([("offset", json!(offset)), ("tag", json!(tag))]),
    )
}

/// `replay.truncated {offset}`.
pub fn replay_truncated(offset: usize) -> Problem {
    Problem::new(
        "replay.truncated",
        format!("The replay ends at offset {offset}, before its start snapshot is whole."),
        detail([("offset", json!(offset))]),
    )
}

/// `replay.write_refused {segment, tick, write, error}`.
pub fn write_refused(segment: u32, tick: u64, write: Value, error: &Problem) -> Problem {
    Problem::new(
        "replay.write_refused",
        format!(
            "A recorded write of tick {tick} (segment {segment}) is refused on replay ({}): the \
             replaying engine or scripts disagree with the recording.",
            error.code
        ),
        detail([
            ("segment", json!(segment)),
            ("tick", json!(tick)),
            ("write", write),
            ("error", serde_json::to_value(error).unwrap_or(Value::Null)),
        ]),
    )
}

/// `replay.bundle_unavailable {hash, reason}`.
pub fn bundle_unavailable(hash: &str, reason: &str) -> Problem {
    Problem::new(
        "replay.bundle_unavailable",
        format!("The script bundle {hash} cannot be loaded: {reason}."),
        detail([("hash", json!(hash)), ("reason", json!(reason))]),
    )
}

/// `replay.data_unavailable {hash, path}`.
pub fn data_unavailable(hash: &str, path: &str) -> Problem {
    Problem::new(
        "replay.data_unavailable",
        format!("The data file {path} ({hash}) is neither embedded nor in the content store."),
        detail([("hash", json!(hash)), ("path", json!(path))]),
    )
}

/// `replay.out_of_range {segment, tick, first, last}`.
pub fn out_of_range(segment: u32, tick: u64, first: u64, last: u64) -> Problem {
    Problem::new(
        "replay.out_of_range",
        format!(
            "Tick {tick} of segment {segment} is outside the replay's ticks {first} to {last}."
        ),
        detail([
            ("segment", json!(segment)),
            ("tick", json!(tick)),
            ("first", json!(first)),
            ("last", json!(last)),
        ]),
    )
}

/// `replay.tainted {segment, tick, reason}`.
pub fn tainted(segment: u32, tick: u64, reason: &str) -> Problem {
    Problem::new(
        "replay.tainted",
        format!(
            "From tick {tick} (segment {segment}) the recording does not reproduce its world: \
             {reason}."
        ),
        detail([
            ("segment", json!(segment)),
            ("tick", json!(tick)),
            ("reason", json!(reason)),
        ]),
    )
}

/// `replay.segment_open {tick}`: a rebase before the open segment was ended (replay.md 2.5).
pub fn segment_open(tick: u64) -> Problem {
    Problem::new(
        "replay.segment_open",
        format!(
            "The recording's segment is still open at tick {tick}: call end_segment with the world \
             as it was before replacing it."
        ),
        detail([("tick", json!(tick))]),
    )
}

/// `replay.segment_closed {tick}`: a tick ended while no segment is open (after `end_segment` or
/// a fault, before `rebase`).
pub fn segment_closed(tick: u64) -> Problem {
    Problem::new(
        "replay.segment_closed",
        format!(
            "Tick {tick} ended but the recording's segment was closed (by end_segment or a              fault): call rebase with the world a restore or a reset made first."
        ),
        detail([("tick", json!(tick))]),
    )
}

/// `replay.tick_sequence {expected, found}`: a tick that does not follow the last one recorded
/// (the world was replaced without `end_segment` and `rebase`).
pub fn tick_sequence(expected: u64, found: u64) -> Problem {
    Problem::new(
        "replay.tick_sequence",
        format!(
            "The recording expected tick {expected} and the world completed tick {found}: a              restore or a reset replaced the world without end_segment and rebase."
        ),
        detail([("expected", json!(expected)), ("found", json!(found))]),
    )
}

/// `version.format {kind, found, supported}`.
pub fn version_format(kind: &str, found: u32, supported: u32) -> Problem {
    Problem::new(
        "version.format",
        format!("This {kind} has format {found}; this engine reads format {supported}."),
        detail([
            ("kind", json!(kind)),
            ("found", json!(found)),
            ("supported", json!(supported)),
        ]),
    )
}

/// `version.mismatch`, the whole comparison as detail.
pub fn version_mismatch(comparison: Detail, what: &[String]) -> Problem {
    Problem::new(
        "version.mismatch",
        format!(
            "Verify needs the recording's versions and these differ: {}; use Compare or Rerun.",
            what.join(", ")
        ),
        comparison,
    )
}

/// `version.schema_differs {sections, suggestion}`.
pub fn schema_differs(sections: &[String]) -> Problem {
    Problem::new(
        "version.schema_differs",
        format!(
            "Schemas or cache identities differ in {}, so every tick hash would differ.",
            sections.join(", ")
        ),
        detail([
            ("sections", json!(sections)),
            (
                "suggestion",
                json!("replay in Rerun mode, or run lockstep with bundles of matching components"),
            ),
        ]),
    )
}

/// `version.newer {section, found, engine}`.
pub fn newer(section: &str, found: u32, engine: u32) -> Problem {
    Problem::new(
        "version.newer",
        format!("Section {section} is at version {found}, newer than this engine's {engine}."),
        detail([
            ("section", json!(section)),
            ("found", json!(found)),
            ("engine", json!(engine)),
        ]),
    )
}

/// `version.fingerprint_mismatch {section, version}`.
pub fn fingerprint_mismatch(section: &str, version: u32) -> Problem {
    Problem::new(
        "version.fingerprint_mismatch",
        format!("Section {section} has version {version} but another shape than this engine's."),
        detail([("section", json!(section)), ("version", json!(version))]),
    )
}

/// `version.unknown_section {section}`.
pub fn version_unknown_section(section: &str) -> Problem {
    Problem::new(
        "version.unknown_section",
        format!("Section {section} is neither registered nor retired."),
        detail([("section", json!(section))]),
    )
}

/// `migrate.missing_step {section, from}`.
pub fn missing_step(section: &str, from: u32) -> Problem {
    Problem::new(
        "migrate.missing_step",
        format!(
            "No migration takes {section} from version {from} to {}.",
            from + 1
        ),
        detail([("section", json!(section)), ("from", json!(from))]),
    )
}

/// `migrate.failed {section, entity, from, path, message, source}`.
pub fn migrate_failed(
    section: &str,
    entity: Option<u64>,
    from: u32,
    path: &str,
    message: &str,
) -> Problem {
    Problem::new(
        "migrate.failed",
        format!("Migrating {section} from version {from} failed at '{path}': {message}."),
        detail([
            ("section", json!(section)),
            ("entity", json!(entity)),
            ("from", json!(from)),
            ("path", json!(path)),
            ("message", json!(message)),
            ("source", Value::Null),
        ]),
    )
}

/// `migrate.shape {section, entity, path, expected, found}`.
pub fn migrate_shape(
    section: &str,
    entity: Option<u64>,
    path: &str,
    expected: &str,
    found: &str,
) -> Problem {
    Problem::new(
        "migrate.shape",
        format!(
            "A migrated {section} value does not fit at '{path}': expected {expected}, found {found}."
        ),
        detail([
            ("section", json!(section)),
            ("entity", json!(entity)),
            ("path", json!(path)),
            ("expected", json!(expected)),
            ("found", json!(found)),
        ]),
    )
}

/// `migrate.nonfinite {section, entity, path}`.
pub fn nonfinite(section: &str, entity: Option<u64>, path: &str) -> Problem {
    Problem::new(
        "migrate.nonfinite",
        format!("A {section} value holds NaN or an infinity at '{path}', which JSON cannot carry."),
        detail([
            ("section", json!(section)),
            ("entity", json!(entity)),
            ("path", json!(path)),
        ]),
    )
}

/// `migrate.untested {section, from, expected, actual}`.
pub fn untested(section: &str, from: u32, expected: Value, actual: Value) -> Problem {
    Problem::new(
        "migrate.untested",
        format!("The migration of {section} from version {from} has no example or one that fails."),
        detail([
            ("section", json!(section)),
            ("from", json!(from)),
            ("expected", expected),
            ("actual", actual),
        ]),
    )
}
