//! Versions, saves and migrations (docs/spec/versions.md 10): V1 (the schema lock), V2 (migration
//! examples), V3 (save round trip), V8 (cache rebuild), V10 (the conversion of a step's result),
//! and the refusals of 6.2.

mod common;

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;
use common::{Bag, Counter, Score, Tag, Vel, game, inputs, run};
use pocket_persist::format::json::from_json;
use pocket_persist::format::trace;
use pocket_persist::lock::{SchemaLock, check};
use pocket_persist::migrate::{Apply, MigrationError, MigrationStep, Migrations, Retired};
use pocket_persist::save::{NoProjectMigrator, load_save, rebuild_caches, write_save};
use pocket_persist::{
    FormatTable, Registry, RestoreOptions, SectionKey, SectionKind, pce, restore, sim_registry,
    snapshot, world_hash,
};
use pocket_sim::{NoHooks, Persisted, RegisterPersisted};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// `Pos` at version 2: a height added.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Pos")]
struct PosV2 {
    x: f64,
    y: f64,
    height: f64,
}
impl Persisted for PosV2 {
    const NAME: &'static str = "Pos";
    const VERSION: u32 = 2;
}

/// `Pos` with another shape and the old version: an unbumped change.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Pos")]
struct PosUnbumped {
    x: f64,
    y: f64,
    reef: u8,
}
impl Persisted for PosUnbumped {
    const NAME: &'static str = "Pos";
    const VERSION: u32 = 1;
}

/// The score under its new name.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Points(i64);
impl Persisted for Points {
    const NAME: &'static str = "Points";
    const VERSION: u32 = 1;
}

fn rest<R: RegisterPersisted>(r: &mut R) {
    r.component::<Vel>()
        .component::<Tag>()
        .component::<Bag>()
        .resource::<Score>()
        .cache::<Counter>();
}

fn reg_with(f: impl FnOnce(&mut pocket_persist::RegistryBuilder)) -> Registry {
    let mut b = sim_registry();
    f(&mut b);
    b.build().unwrap()
}

fn add_height(
    _: &pocket_persist::migrate::MigrationCtx,
    mut v: Value,
) -> Result<Value, MigrationError> {
    let (x, y) = (v["x"].as_f64(), v["y"].as_f64());
    let (Some(x), Some(y)) = (x, y) else {
        return Err(MigrationError::field("/x"));
    };
    v.as_object_mut()
        .ok_or_else(|| MigrationError::field(""))?
        .insert("height".into(), json!(x + y));
    Ok(v)
}

fn pos_step() -> MigrationStep {
    MigrationStep {
        kind: SectionKind::Component,
        section: "Pos",
        from: 1,
        apply: Apply::Value(add_height),
        examples: &[(r#"{"x":1.0,"y":2.0}"#, r#"{"x":1.0,"y":2.0,"height":3.0}"#)],
    }
}

fn saved() -> (Vec<u8>, common::Game) {
    let mut g = game(31);
    run(&mut g, None, &inputs(), 40);
    let mut meta = BTreeMap::new();
    meta.insert("scenario".into(), "test".into());
    (
        write_save(g.sim.world(), &g.reg, "slot 1", meta).unwrap(),
        g,
    )
}

/// V3: `write_save`, `load_save` and `restore` in the same engine give the original snapshot byte
/// for byte, and the restored world continues in lockstep with the original.
#[test]
fn v3_save_round_trip() {
    let (bytes, mut a) = saved();
    let mut b = game(1);
    let (snap, report) = load_save(
        &bytes,
        &b.reg,
        &Migrations::default(),
        &mut NoProjectMigrator,
        b.sim.world(),
    )
    .unwrap();
    assert!(
        report.migrated.is_empty() && report.caches_rebuilt.is_empty() && !report.engine_differs
    );
    let original = snapshot(a.sim.world(), &a.reg).unwrap();
    assert_eq!(snap.to_bytes(), original.to_bytes());
    restore(b.sim.world_mut(), &snap, &b.reg, RestoreOptions::default()).unwrap();
    for _ in 0..600 {
        a.sim.step(&mut NoHooks).unwrap();
        b.sim.step(&mut NoHooks).unwrap();
        assert_eq!(
            world_hash(a.sim.world(), &a.reg).unwrap(),
            world_hash(b.sim.world(), &b.reg).unwrap()
        );
    }
}

/// V8: a save whose cache identity differs loads with the cache listed in `caches_rebuilt`; the
/// rebuilt world runs 600 ticks.
#[test]
fn v8_cache_rebuild() {
    let (bytes, _) = saved();
    // Alter the saved identity in the format table: decode, edit, re-encode the save.
    let mut d = pce::Decoder::new(&bytes, false);
    d.take(12).unwrap();
    let hlen = d.uleb().unwrap() as usize;
    let header = d.take(hlen).unwrap().to_vec();
    let tlen = d.uleb().unwrap() as usize;
    let mut table: FormatTable = pce::from_bytes(d.take(tlen).unwrap(), false).unwrap();
    let snap = bytes[d.pos()..].to_vec();
    let key = SectionKey::new(SectionKind::Cache, "test.counter");
    table
        .entries
        .iter_mut()
        .find(|e| e.section == key)
        .unwrap()
        .identity = Some("test counter layout 0".into());
    let mut edited = bytes[..12].to_vec();
    for part in [header, pce::to_bytes(&table).unwrap()] {
        pce::write_uleb(&mut edited, part.len() as u64);
        edited.extend_from_slice(&part);
    }
    edited.extend_from_slice(&snap);
    let mut g = game(1);
    let (s, report) = load_save(
        &edited,
        &g.reg,
        &Migrations::default(),
        &mut NoProjectMigrator,
        g.sim.world(),
    )
    .unwrap();
    assert_eq!(report.caches_rebuilt, vec![key.clone()]);
    assert!(s.section(&key).is_none());
    restore(g.sim.world_mut(), &s, &g.reg, RestoreOptions::default()).unwrap();
    assert!(g.sim.world().get_resource::<Counter>().is_none());
    rebuild_caches(g.sim.world_mut(), &g.reg, &report.caches_rebuilt).unwrap();
    assert_eq!(g.sim.world().resource::<Counter>().hits, 40);
    for _ in 0..600 {
        g.sim.step(&mut NoHooks).unwrap();
    }
    assert_eq!(g.sim.world().resource::<Counter>().hits, 640);
}

/// A save at version 1 loads into an engine at version 2 through the step's JSON, and every
/// refusal of 6.2 has its code.
#[test]
fn migrations_and_refusals() {
    let (bytes, g) = saved();
    let v2 = reg_with(|b| {
        b.component::<PosV2>();
        rest(b);
    });
    let mut w = World::new();
    let steps = Migrations::new(vec![pos_step()], Vec::new());
    let (snap, report) = load_save(&bytes, &v2, &steps, &mut NoProjectMigrator, &w).unwrap();
    let pos = SectionKey::new(SectionKind::Component, "Pos");
    assert_eq!(report.migrated, vec![(pos.clone(), 1, 2)]);
    restore(&mut w, &snap, &v2, RestoreOptions::default()).unwrap();
    let mut q = w.query::<(&pocket_sim::EntityId, &PosV2)>();
    #[allow(clippy::disallowed_methods)] // every row is checked; the order changes nothing
    let rows: Vec<(u64, PosV2)> = q.iter(&w).map(|(i, p)| (i.get(), *p)).collect();
    assert!(!rows.is_empty());
    for (id, p) in &rows {
        assert_eq!(p.height, p.x + p.y, "entity {id}");
        let e = common::ent(&g.sim, *id).unwrap();
        assert_eq!(g.sim.world().get::<common::Pos>(e).unwrap().x, p.x);
    }
    let code = |r: Result<_, pocket_contract::Problem>| r.map(drop).unwrap_err().code;
    assert_eq!(
        code(load_save(
            &bytes,
            &v2,
            &Migrations::default(),
            &mut NoProjectMigrator,
            &w
        )),
        "migrate.missing_step"
    );
    // A save written at version 2 into the version 1 engine.
    let save2 = write_save(&w, &v2, "v2", BTreeMap::new()).unwrap();
    assert_eq!(
        code(load_save(
            &save2,
            &g.reg,
            &steps,
            &mut NoProjectMigrator,
            &w
        )),
        "version.newer"
    );
    let unbumped = reg_with(|b| {
        b.component::<PosUnbumped>();
        rest(b);
    });
    assert_eq!(
        code(load_save(
            &bytes,
            &unbumped,
            &steps,
            &mut NoProjectMigrator,
            &w
        )),
        "version.fingerprint_mismatch"
    );
    let no_score = reg_with(|b| {
        b.component::<common::Pos>()
            .component::<Vel>()
            .component::<Tag>()
            .component::<Bag>()
            .cache::<Counter>();
    });
    assert_eq!(
        code(load_save(
            &bytes,
            &no_score,
            &steps,
            &mut NoProjectMigrator,
            &w
        )),
        "version.unknown_section"
    );
    let removed = Migrations::new(
        Vec::new(),
        vec![Retired::Removed {
            kind: SectionKind::Resource,
            section: "Score",
            last_version: 1,
        }],
    );
    let (_, r) = load_save(&bytes, &no_score, &removed, &mut NoProjectMigrator, &w).unwrap();
    assert_eq!(
        r.dropped,
        vec![SectionKey::new(SectionKind::Resource, "Score")]
    );
    let points = reg_with(|b| {
        b.component::<common::Pos>()
            .component::<Vel>()
            .component::<Tag>()
            .component::<Bag>()
            .resource::<Points>()
            .cache::<Counter>();
    });
    let renamed = Migrations::new(
        Vec::new(),
        vec![Retired::Renamed {
            kind: SectionKind::Resource,
            from: "Score",
            to: "Points",
            at_version: 1,
        }],
    );
    let (s, r) = load_save(&bytes, &points, &renamed, &mut NoProjectMigrator, &w).unwrap();
    assert_eq!(r.renamed, vec![("Score".to_owned(), "Points".to_owned())]);
    let mut w2 = World::new();
    restore(&mut w2, &s, &points, RestoreOptions::default()).unwrap();
    assert_eq!(
        w2.resource::<Points>().0,
        g.sim.world().resource::<Score>().0
    );
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Meter {
    level: f64,
    count: u32,
    base: f64,
}
impl Persisted for Meter {
    const NAME: &'static str = "Meter";
    const VERSION: u32 = 2;
}

/// V10: a step's `-0`, `5.0` for a `u32` and integer `0` for an `f64` convert to the expected PCE
/// bytes; a step returning NaN fails with `migrate.nonfinite`. V2: examples are run, and a step
/// without examples or with a failing one is `migrate.untested`.
#[test]
fn v10_conversion_and_v2_examples() {
    let f = trace::<Meter>().unwrap();
    let mut got = Vec::new();
    from_json(
        &f,
        &json!({"level": -0.0, "count": 5.0, "base": 0}),
        &mut got,
    )
    .unwrap();
    let want = pce::to_bytes(&Meter {
        level: -0.0,
        count: 5,
        base: 0.0,
    })
    .unwrap();
    assert_eq!(got, want);
    assert_eq!(
        f64::from_bits(u64::from_le_bytes(got[..8].try_into().unwrap())).to_bits(),
        (-0.0f64).to_bits()
    );
    let nan = json!({"level": f64::NAN, "count": 1, "base": 0.0});
    let e = from_json(&f, &nan, &mut Vec::new()).unwrap_err();
    let p = pocket_persist::migrate::conversion_problem("component:Meter", None, &e);
    assert_eq!(p.code, "migrate.nonfinite");
    let e = from_json(
        &f,
        &json!({"level": 1.0, "count": 2.5, "base": 0.0}),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(
        pocket_persist::migrate::conversion_problem("component:Meter", None, &e).code,
        "migrate.shape"
    );

    fn to_meter(
        _: &pocket_persist::migrate::MigrationCtx,
        v: Value,
    ) -> Result<Value, MigrationError> {
        Ok(json!({"level": v["level"], "count": 5.0, "base": 0}))
    }
    let meter = SectionKey::new(SectionKind::Component, "Meter");
    let current = |k: &SectionKey| (*k == meter).then(|| (2, trace::<Meter>().unwrap()));
    let good = MigrationStep {
        kind: SectionKind::Component,
        section: "Meter",
        from: 1,
        apply: Apply::Value(to_meter),
        examples: &[(
            r#"{"level":-0.0}"#,
            r#"{"level":-0.0,"count":5,"base":0.0}"#,
        )],
    };
    assert!(
        Migrations::new(vec![good], Vec::new())
            .check_examples(&current)
            .is_empty()
    );
    let wrong = MigrationStep {
        kind: SectionKind::Component,
        section: "Meter",
        from: 1,
        apply: Apply::Value(to_meter),
        examples: &[(r#"{"level":0.0}"#, r#"{"level":-0.0,"count":5,"base":0.0}"#)],
    };
    let none = MigrationStep {
        kind: SectionKind::Component,
        section: "Meter",
        from: 1,
        apply: Apply::Value(to_meter),
        examples: &[],
    };
    let problems = Migrations::new(vec![wrong, none], Vec::new()).check_examples(&current);
    assert_eq!(
        problems.iter().map(|p| p.code.as_str()).collect::<Vec<_>>(),
        ["migrate.untested", "migrate.untested"]
    );
}

/// V1: every rule of the schema lock's table.
#[test]
fn v1_schema_lock() {
    let g = game(1);
    let table = |reg: &Registry| FormatTable::of(reg, g.sim.world()).unwrap();
    let lock = SchemaLock::from_text(&SchemaLock::of(&table(&g.reg)).to_text()).unwrap();
    assert!(check(&table(&g.reg), &lock, &Migrations::default()).is_empty());
    let codes = |reg: &Registry, m: &Migrations| -> Vec<String> {
        check(&table(reg), &lock, m)
            .into_iter()
            .map(|p| p.code)
            .collect()
    };
    let unbumped = reg_with(|b| {
        b.component::<PosUnbumped>();
        rest(b);
    });
    let p = check(&table(&unbumped), &lock, &Migrations::default());
    assert_eq!(p[0].code, "version.unbumped");
    assert_eq!(p[0].detail["changes"], json!(["/reef added"]));
    let v2 = reg_with(|b| {
        b.component::<PosV2>();
        rest(b);
    });
    assert_eq!(
        codes(&v2, &Migrations::new(vec![pos_step()], Vec::new())),
        ["version.lock_stale"]
    );
    assert_eq!(codes(&v2, &Migrations::default()), ["migrate.missing_step"]);
    let mut downgraded = lock.clone();
    downgraded
        .sections
        .iter_mut()
        .find(|e| e.section.name == "Pos")
        .unwrap()
        .version = 3;
    assert_eq!(
        check(&table(&g.reg), &downgraded, &Migrations::default())[0].code,
        "version.downgrade"
    );
    let with_points = reg_with(|b| {
        b.component::<common::Pos>();
        rest(b);
        b.resource::<Points>();
    });
    assert_eq!(
        codes(&with_points, &Migrations::default()),
        ["version.lock_stale"]
    );
    let no_score = reg_with(|b| {
        b.component::<common::Pos>()
            .component::<Vel>()
            .component::<Tag>()
            .component::<Bag>()
            .cache::<Counter>();
    });
    assert_eq!(
        codes(&no_score, &Migrations::default()),
        ["migrate.missing_removal"]
    );
    let removed = Migrations::new(
        Vec::new(),
        vec![Retired::Removed {
            kind: SectionKind::Resource,
            section: "Score",
            last_version: 1,
        }],
    );
    assert!(codes(&no_score, &removed).is_empty());
}
