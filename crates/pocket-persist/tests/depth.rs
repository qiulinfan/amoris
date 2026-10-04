//! Nesting depth (docs/spec/persistence.md 14, choice 21): the encoder and the decoder refuse a
//! value nested more than `pce::MAX_DEPTH` compound values deep, so neither a value a script built
//! nor bytes from a file can recurse until the stack overflows; the JSON conversion and the field
//! diff hold to the same bound. Run in debug and in release.

mod common;

use pocket_persist::format::json::{from_json, to_json};
use pocket_persist::hash::{digest_prefix, section_digest};
use pocket_persist::pce::{self, MAX_DEPTH, PceError};
use pocket_persist::{
    RestoreOptions, SectionData, SectionKey, SectionKind, Snapshot, diff, restore, snapshot,
    world_hash,
};
use pocket_sim::{EventKind, NewEvent, PlainData};
use serde_json::json;

/// `n` arrays around `Null`, built without recursion.
fn nested(n: usize) -> PlainData {
    let mut v = PlainData::Null;
    for _ in 0..n {
        v = PlainData::Array(vec![v]);
    }
    v
}

/// Takes a nested value apart without recursion (its drop glue would recurse).
fn dismantle(mut v: PlainData) {
    while let PlainData::Array(mut a) = v {
        match a.pop() {
            Some(inner) => v = inner,
            None => break,
        }
    }
}

/// `n` arrays around `Null` as PCE bytes: variant 4 (`Array`), length 1, ..., variant 0 (`Null`).
fn nested_bytes(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 * n + 1);
    for _ in 0..n {
        out.extend_from_slice(&[4, 1]);
    }
    out.push(0);
    out
}

/// An array is two levels (its newtype variant and its sequence) and `Null` none, so 64 arrays sit
/// exactly at the bound: they round trip, and 65 are refused both ways.
#[test]
fn the_bound_is_the_same_both_ways() {
    let levels = MAX_DEPTH as usize / 2;
    let at = nested(levels);
    let bytes = pce::to_bytes(&at).unwrap();
    assert_eq!(bytes, nested_bytes(levels));
    let back: PlainData = pce::from_bytes(&bytes, true).unwrap();
    assert!(back == at);
    dismantle(back);
    dismantle(at);

    let past = nested(levels + 1);
    let e = pce::to_bytes(&past).unwrap_err();
    assert!(
        matches!(&e, PceError::Encode(r) if r.contains("nesting deeper than 128")),
        "{e}"
    );
    dismantle(past);
    let e = pce::from_bytes::<PlainData>(&nested_bytes(levels + 1), true).unwrap_err();
    assert!(
        matches!(&e, PceError::Noncanonical { reason, .. } if reason.contains("nesting deeper")),
        "{e}"
    );
}

/// A value 100,000 levels deep is refused by the encoder, not written until the stack overflows.
#[test]
fn a_very_deep_value_is_refused_by_the_encoder() {
    let deep = nested(100_000);
    let e = pce::to_bytes(&deep).unwrap_err();
    assert!(
        matches!(&e, PceError::Encode(r) if r.contains("nesting deeper")),
        "{e}"
    );
    dismantle(deep);
}

/// The inbox of a world whose boundary emitted one event with `Null` data, its snapshot, and the
/// section with that `Null` replaced by `n` nested arrays (the section's digest recomputed, so the
/// snapshot parses as one from a file would).
fn deep_inbox(n: usize) -> (common::Game, Snapshot, Snapshot) {
    let mut g = common::game(3);
    pocket_sim::event::emit(
        g.sim.world_mut(),
        NewEvent::new(EventKind::new("test.deep").unwrap()),
    );
    let good = snapshot(g.sim.world(), &g.reg).unwrap();
    let key = SectionKey::new(SectionKind::Resource, "EventInbox");
    let sections: Vec<SectionData> = good
        .sections()
        .iter()
        .map(|s| {
            if s.key != key {
                return s.clone();
            }
            // The event's data (`Null`, one byte) is followed by `boundary_from` (a u32).
            let at = s.bytes.len() - 5;
            assert_eq!(s.bytes[at], 0);
            let mut bytes = s.bytes[..at].to_vec();
            bytes.extend_from_slice(&nested_bytes(n));
            bytes.extend_from_slice(&s.bytes[at + 1..]);
            let mut buf = Vec::new();
            digest_prefix(&mut buf, s.key.kind, &s.key.name, s.version);
            buf.extend_from_slice(&bytes);
            SectionData {
                digest: section_digest(&buf),
                bytes: bytes.into(),
                ..s.clone()
            }
        })
        .collect();
    let deep = Snapshot::from_parts(good.header().clone(), sections);
    let deep = Snapshot::from_bytes(&deep.to_bytes()).expect("the framing is sound");
    (g, good, deep)
}

/// A snapshot from a file whose event data is nested 100,000 deep (200 KB) parses (no section is
/// decoded) and is refused by restore with `persist.noncanonical`, the world unchanged; the JSON
/// conversion and the field diff refuse it too.
#[test]
fn deep_bytes_from_a_file_are_refused() {
    let (mut g, good, deep) = deep_inbox(100_000);
    let e = restore(g.sim.world_mut(), &deep, &g.reg, RestoreOptions::default()).unwrap_err();
    assert_eq!(e.code, "persist.noncanonical");
    assert!(
        e.message.contains("nesting deeper than 128"),
        "{}",
        e.message
    );
    assert_eq!(snapshot(g.sim.world(), &g.reg).unwrap(), good);

    let key = SectionKey::new(SectionKind::Resource, "EventInbox");
    let format = g.reg.entry(&key).unwrap().format.clone().unwrap();
    let bytes = &deep.section(&key).unwrap().bytes;
    assert!(to_json(&format, bytes, false).is_err());
    // The field diff refuses the section rather than recursing.
    let e = diff(&good, &deep, &g.reg, 50).unwrap_err();
    assert_eq!(e.code, "persist.noncanonical");

    // JSON from a migration step nested 300 deep is refused by the conversion back.
    let mut v = to_json(&format, &good.section(&key).unwrap().bytes, true).unwrap();
    let mut data = json!("Null");
    for _ in 0..300 {
        data = json!({ "Array": [data] });
    }
    v["events"][0]["data"] = data;
    let mut out = Vec::new();
    assert!(from_json(&format, &v, &mut out).is_err());
}

/// Event data a script emitted 100,000 deep fails the world hash with `persist.encode`. Run on a
/// thread with a large stack: building, validating and dropping such a value recurses outside this
/// crate.
#[test]
fn a_deep_event_fails_the_hash() {
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(|| {
            let mut g = common::game(3);
            pocket_sim::event::emit(
                g.sim.world_mut(),
                NewEvent::new(EventKind::new("test.deep").unwrap()).data(nested(100_000)),
            );
            let e = world_hash(g.sim.world(), &g.reg).unwrap_err();
            assert_eq!(e.code, "persist.encode");
            assert!(e.message.contains("nesting deeper"), "{}", e.message);
        })
        .unwrap()
        .join()
        .unwrap();
}

/// At the deepest event data the bound accepts (62 arrays inside the inbox's three levels), restore,
/// the JSON round trip and the field diff succeed on a 1 MiB stack (the main thread's on Windows).
/// Measured: 512 KiB suffice in a debug build and 256 KiB in release.
#[test]
fn the_deepest_accepted_value_fits_a_small_stack() {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let (mut g, good, deep) = deep_inbox(62);
            let key = SectionKey::new(SectionKind::Resource, "EventInbox");
            let format = g.reg.entry(&key).unwrap().format.clone().unwrap();
            let bytes = &deep.section(&key).unwrap().bytes;
            let v = to_json(&format, bytes, true).unwrap();
            let mut out = Vec::new();
            from_json(&format, &v, &mut out).unwrap();
            assert_eq!(&out[..], &bytes[..]);
            let d = diff(&good, &deep, &g.reg, 50).unwrap();
            assert_eq!(d.fields.len(), 1);
            restore(g.sim.world_mut(), &deep, &g.reg, RestoreOptions::default()).unwrap();
            assert_eq!(snapshot(g.sim.world(), &g.reg).unwrap(), deep);
            // One array more is past the bound.
            let (mut g, _, deeper) = deep_inbox(63);
            let e = restore(
                g.sim.world_mut(),
                &deeper,
                &g.reg,
                RestoreOptions::default(),
            )
            .unwrap_err();
            assert_eq!(e.code, "persist.noncanonical");
        })
        .unwrap()
        .join()
        .unwrap();
}
