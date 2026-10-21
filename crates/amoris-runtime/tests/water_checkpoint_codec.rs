//! Checked immutable checkpoints keep the existing persisted field format and stage bad input.
use amoris_assets::water_field::FieldCheckpoint;
use amoris_persist::{Registry, RestoreOptions, SectionKey, SectionKind, restore, snapshot};
use amoris_physics::water::resident::WaterCheckpoints;
use amoris_sim::{ContentHash, NoHooks, Persisted, RegisterPersisted, Sim, SimConfig, TickRate};
use bevy_ecs::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Resource, Serialize, Deserialize)]
#[serde(rename = "WaterCheckpoints")]
struct Legacy {
    fields: BTreeMap<ContentHash, Arc<FieldCheckpoint>>,
}
impl Persisted for Legacy {
    const NAME: &'static str = "WaterCheckpoints";
    const VERSION: u32 = 2;
}
fn registry<T: Persisted + Resource>() -> Registry {
    let mut r = amoris_persist::sim_registry();
    r.resource::<T>();
    r.build().unwrap()
}
fn world() -> Sim {
    Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 1,
    })
    .unwrap()
}
fn checkpoints() -> BTreeMap<ContentHash, Arc<FieldCheckpoint>> {
    [[3, 5], [7, 3]]
        .into_iter()
        .enumerate()
        .map(|(region, size)| {
            let columns = (0..size[0] * size[1])
                .map(|i| {
                    [
                        i as f32 / 8.0 - 4.0,
                        i as f32 / 16.0 + region as f32 + 0.25,
                        -(i as f32) / 4.0,
                        i as f32 / 2.0,
                    ]
                    .map(f32::to_bits)
                })
                .collect();
            let mut raw = FieldCheckpoint::new(size, if region == 0 { -0.0 } else { 7.5 }, columns);
            raw.columns[0] = [(-0.0f32).to_bits(), 0, 0, (-0.0f32).to_bits()];
            for (i, face) in raw.far_faces.iter_mut().enumerate() {
                *face = ((i as f32 + 1.0) / -8.0).to_bits();
            }
            raw.far_faces[0] = (-0.0f32).to_bits();
            (raw.key(), Arc::new(raw))
        })
        .collect()
}
fn checked(fields: &BTreeMap<ContentHash, Arc<FieldCheckpoint>>) -> WaterCheckpoints {
    WaterCheckpoints {
        fields: fields
            .iter()
            .map(|(&key, raw)| (key, Arc::new(raw.as_ref().clone().checked().unwrap())))
            .collect(),
    }
}
#[test]
fn resource_fingerprint_and_canonical_bytes_are_the_legacy_values() {
    let fields = checkpoints();
    assert_eq!(fields.len(), 2);
    let mut legacy = world();
    legacy.world_mut().insert_resource(Legacy {
        fields: fields.clone(),
    });
    let mut current = world();
    current.world_mut().insert_resource(checked(&fields));
    let a = registry::<Legacy>();
    let b = registry::<WaterCheckpoints>();
    let section = SectionKey::new(SectionKind::Resource, "WaterCheckpoints");
    assert_eq!(
        a.entry(&section).unwrap().fingerprint,
        b.entry(&section).unwrap().fingerprint
    );
    let old = snapshot(legacy.world(), &a).unwrap();
    let encoded = snapshot(current.world(), &b).unwrap();
    assert_eq!(
        old.to_bytes(),
        encoded.to_bytes(),
        "cached identities changed persisted data"
    );
    restore(current.world_mut(), &old, &b, RestoreOptions::default()).unwrap();
    let restored = current.world().resource::<WaterCheckpoints>();
    assert_eq!(restored.fields.len(), fields.len());
    for (&key, raw) in &fields {
        let value = &restored.fields[&key];
        assert_eq!(value.size, raw.size);
        assert_eq!(value.datum.to_bits(), raw.datum.to_bits());
        assert_eq!(value.columns, raw.columns);
        assert_eq!(value.far_faces, raw.far_faces);
        assert_eq!(value.key(), key);
        assert_eq!(value.digest(), raw.digest());

        // An editable copy must not change the decoded immutable checkpoint or its identity.
        let mut editable = value.as_ref().clone().into_raw();
        editable.columns[0][1] = 2.0f32.to_bits();
        editable.far_faces[0] = 0.5f32.to_bits();
        let edited = editable.checked().unwrap();
        assert_ne!(edited.key(), key);
        assert_ne!(edited.digest(), value.digest());
        assert_eq!(value.as_raw(), raw.as_ref());
        assert_eq!(value.key(), key);
        assert_eq!(value.digest(), raw.digest());
    }
    assert_eq!(
        snapshot(current.world(), &b).unwrap().to_bytes(),
        old.to_bytes()
    );
}
#[test]
fn malformed_legacy_data_is_rejected_before_mutating_a_restored_world() {
    let fields = checkpoints();
    let mut current = world();
    current.world_mut().insert_resource(checked(&fields));
    let r = registry::<WaterCheckpoints>();
    let before = snapshot(current.world(), &r).unwrap();
    for corruption in ["column NaN", "far-face NaN", "missing far face"] {
        let mut bad = world();
        // The incoming clock and valid checkpoint precede the bad payload during staging.
        bad.step(&mut NoHooks).unwrap();
        let mut malformed = fields.clone();
        {
            let mut last = malformed.last_entry().unwrap();
            let data = Arc::make_mut(last.get_mut());
            match corruption {
                "column NaN" => data.columns[1][1] = f32::NAN.to_bits(),
                "far-face NaN" => *data.far_faces.last_mut().unwrap() = f32::NAN.to_bits(),
                "missing far face" => {
                    data.far_faces.pop();
                }
                _ => unreachable!(),
            }
        }
        bad.world_mut()
            .insert_resource(Legacy { fields: malformed });
        let invalid = snapshot(bad.world(), &registry::<Legacy>()).unwrap();
        assert!(
            restore(current.world_mut(), &invalid, &r, RestoreOptions::default()).is_err(),
            "accepted {corruption}"
        );
        assert_eq!(
            snapshot(current.world(), &r).unwrap().to_bytes(),
            before.to_bytes(),
            "restore changed the world after {corruption}"
        );
    }
}
