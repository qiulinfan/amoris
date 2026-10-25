//! Immutable validity/identity witnesses never serialize their cached metadata.
use amoris_assets::water_field::{
    CheckedCheckpoint, ColumnDelta, FieldBoundary, FieldCheckpoint, FieldRecipe, FieldStep,
    MAX_STEPS,
};
fn raw() -> FieldCheckpoint {
    let mut c = FieldCheckpoint::new(
        [17, 18],
        1234.125,
        (0..17 * 18)
            .map(|n| {
                [
                    (n as f32 * 0.001).to_bits(),
                    2.5f32.to_bits(),
                    0.125f32.to_bits(),
                    (-0.25f32).to_bits(),
                ]
            })
            .collect(),
    );
    c.far_faces[0] = (-0.0f32).to_bits();
    c.far_faces[1] = 3.0f32.to_bits();
    c
}
#[test]
fn checked_identity_is_bound_to_immutable_values_and_preserves_raw_wire_bytes() {
    let input = raw();
    let expected = input.identity();
    let encoded = bincode2::serde::encode_to_vec(&input, bincode2::config::standard()).unwrap();
    let checked = input.clone().checked().unwrap();
    assert_eq!(checked.identity(), expected);
    assert_eq!(
        bincode2::serde::encode_to_vec(&checked, bincode2::config::standard()).unwrap(),
        encoded
    );
    assert_eq!(
        serde_json::to_value(&checked).unwrap(),
        serde_json::to_value(&input).unwrap()
    );
    let (decoded, used): (CheckedCheckpoint, usize) =
        bincode2::serde::decode_from_slice(&encoded, bincode2::config::standard()).unwrap();
    assert_eq!(used, encoded.len());
    assert_eq!(decoded.as_raw(), &input);
    assert_eq!(decoded.identity(), input.identity());
    let mut edited = checked.clone().into_raw();
    edited.columns[0][1] = 3.0f32.to_bits();
    let edited = edited.checked().unwrap();
    assert_ne!(edited.digest(), checked.digest());
    assert_eq!(checked.identity(), expected);
    let mut other = checked.clone().into_raw();
    other.datum = -0.0;
    let negative = other.clone().checked().unwrap();
    other.datum = 0.0;
    let positive = other.checked().unwrap();
    assert_eq!(negative.digest(), positive.digest());
    assert_ne!(negative.key(), positive.key());
}
#[test]
fn every_new_payload_is_checked_including_deserialization() {
    let original = raw();
    let mut bad = Vec::new();
    for channel in 0..4 {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut c = original.clone();
            c.columns[0][channel] = value.to_bits();
            bad.push(c);
        }
    }
    let mut c = original.clone();
    c.columns[0][1] = (-0.01f32).to_bits();
    bad.push(c);
    let mut c = original.clone();
    c.far_faces[0] = f32::NAN.to_bits();
    bad.push(c);
    let mut c = original.clone();
    c.datum = f64::NAN;
    bad.push(c);
    let mut c = original.clone();
    c.size[0] = 513;
    bad.push(c);
    let mut c = original.clone();
    c.columns.pop();
    bad.push(c);
    let mut c = original.clone();
    c.far_faces.pop();
    bad.push(c);
    for c in bad {
        let encoded = bincode2::serde::encode_to_vec(&c, bincode2::config::standard()).unwrap();
        assert!(c.checked().is_err());
        assert!(
            bincode2::serde::decode_from_slice::<CheckedCheckpoint, _>(
                &encoded,
                bincode2::config::standard()
            )
            .is_err()
        );
    }
    let mut signed_zero = original;
    signed_zero.columns[0][1] = (-0.0f32).to_bits();
    assert_eq!(
        signed_zero.clone().checked().unwrap().identity(),
        signed_zero.identity()
    );
}
#[test]
fn checked_and_raw_recipe_validation_have_identical_bounds_and_digest_rules() {
    let raw = raw();
    let checked = raw.clone().checked().unwrap();
    let step = FieldStep {
        sea: None,
        boundaries: [FieldBoundary::Wall; 4],
        cell_m: 1.0,
        substep_dt: 0.01,
        substeps: 1,
        friction: 0.02,
        calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
        dry_below: 0.0,
        edits: vec![],
        impulses: vec![],
        displacements: vec![],
        physics: Default::default(),
    };
    let mut recipes = vec![
        FieldRecipe::new(raw.key(), vec![], raw.digest(), String::new()),
        FieldRecipe::new(raw.key(), vec![step.clone()], [7; 32], String::new()),
    ];
    let mut mismatch = recipes[0].clone();
    mismatch.digest[0] ^= 1;
    recipes.push(mismatch);
    recipes.push(FieldRecipe::new(
        amoris_sim::ContentHash([1; 32]),
        vec![],
        raw.digest(),
        String::new(),
    ));
    recipes.push(FieldRecipe::new(
        raw.key(),
        vec![step.clone(); MAX_STEPS + 1],
        raw.digest(),
        String::new(),
    ));
    let mut bad = step;
    bad.edits.push(ColumnDelta {
        column: raw.columns.len() as u32,
        depth_m: 0.1,
    });
    recipes.push(FieldRecipe::new(
        raw.key(),
        vec![bad],
        [7; 32],
        String::new(),
    ));
    for recipe in recipes {
        assert_eq!(recipe.validate(&raw), recipe.validate_checked(&checked));
    }
}
