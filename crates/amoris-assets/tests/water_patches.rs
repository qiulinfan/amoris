//! Presentation patches preserve exact input order through coalescing, or reject before commit.
use amoris_assets::{
    frame::{RenderFrame, ResidentWaterUpdate, WaterLookView},
    water_field::{ColumnDelta, FieldCheckpoint, FieldRecipe, FieldStep, PointImpulse},
};

fn seed() -> FieldCheckpoint {
    FieldCheckpoint::new(
        [16, 16],
        0.0,
        vec![[0.0f32.to_bits(), 2.0f32.to_bits(), 0, 0]; 256],
    )
}
fn recipe(count: usize) -> FieldRecipe {
    let c = seed();
    FieldRecipe::new(
        c.key(),
        (0..count)
            .map(|n| FieldStep {
                exterior: vec![],
                sea: None,
                boundaries: [amoris_assets::water_field::FieldBoundary::Wall; 4],
                cell_m: 1.0,
                substep_dt: 0.01,
                substeps: 1,
                friction: 0.02,
                calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
                dry_below: 0.0,
                edits: vec![ColumnDelta {
                    column: 17,
                    depth_m: n as f64 * 0.001,
                }],
                impulses: vec![PointImpulse {
                    at: [4.5, 5.5],
                    impulse: [n as f64 + 1.0, -0.25],
                }],
                displacements: vec![],
                physics: Default::default(),
            })
            .collect(),
        if count == 0 {
            c.digest()
        } else {
            [count as u8; 32]
        },
        "test field".into(),
    )
}
fn update(r: &FieldRecipe, start: usize, checkpoint: bool) -> ResidentWaterUpdate {
    ResidentWaterUpdate {
        id: 7,
        origin: [0.0; 2],
        cell_m: 1.0,
        look: WaterLookView::default(),
        checkpoint: checkpoint.then(|| std::sync::Arc::new(seed().checked().unwrap())),
        recipe: r.patch(start),
        edge_level: None,
    }
}
fn frame(r: &FieldRecipe, start: usize, checkpoint: bool) -> RenderFrame {
    RenderFrame {
        tick: r.steps().len() as u64,
        resident_water: vec![update(r, start, checkpoint)],
        ..Default::default()
    }
}
#[test]
fn patches_authenticate_the_prefix_and_compose_without_repeating_history() {
    let three = recipe(3);
    let seven = recipe(7);
    let full = three.patch(0);
    let tail = seven.patch(3);
    assert_eq!(tail.steps.len(), 4);
    assert_eq!(tail.apply(Some(&three)).unwrap(), seven);
    assert_eq!(full.apply(None).unwrap(), three);
    let mut combined = full;
    combined.append(tail.clone()).unwrap();
    assert_eq!(combined, seven.patch(0));
    let mut pending = recipe(5).patch(3);
    pending.append(seven.patch(5)).unwrap();
    assert_eq!(pending, tail);
    let remembered = Some((3, three.program_key()));
    assert_eq!(seven.patch_after(remembered), seven.patch(3));
    let mut changed: Vec<_> = seven.steps().iter().map(|s| (**s).clone()).collect();
    changed[0].friction = 0.5;
    let branch = FieldRecipe::new(
        seven.checkpoint(),
        changed,
        seven.digest,
        seven.environment.clone(),
    );
    assert_eq!(
        branch.patch_after(remembered),
        branch.patch(0),
        "changed prefix requires full replacement"
    );
    let empty = seven.patch(7);
    assert!(empty.steps.is_empty());
    assert_eq!(empty.apply(Some(&seven)).unwrap(), seven);
    assert!(tail.apply(None).is_err());
    assert!(tail.apply(Some(&recipe(2))).is_err());
}
#[test]
fn malformed_or_out_of_order_patches_do_not_change_pending_frames() {
    let mut current = frame(&recipe(3), 0, true);
    let saved = current.clone();
    let mut altered = frame(&recipe(4), 3, false);
    altered.resident_water[0].recipe.steps[0].impulses[0].impulse[0] += 10.0;
    assert!(RenderFrame::decode(&altered.encode()).is_err());
    assert!(current.merge(altered).is_err());
    assert_eq!(current, saved);
    let mut mismatched = frame(&recipe(3), 0, true);
    let mut other = seed();
    other.datum = 0.5;
    mismatched.resident_water[0].checkpoint = Some(other.checked().unwrap().into());
    assert!(RenderFrame::decode(&mismatched.encode()).is_err());
    let mut trailing = current.encode();
    trailing.push(0);
    assert!(RenderFrame::decode(&trailing).is_err());
    assert!(current.merge(frame(&recipe(5), 4, false)).is_err());
    assert_eq!(current, saved);
    let mut wrong = frame(&recipe(4), 3, false);
    wrong.resident_water[0].recipe.environment = "another execution".into();
    assert!(current.merge(wrong).is_err());
    assert_eq!(current, saved);
    let mut duplicated = frame(&recipe(4), 3, false);
    duplicated
        .resident_water
        .push(duplicated.resident_water[0].clone());
    assert!(current.merge(duplicated).is_err());
    assert_eq!(current, saved);
    let mut reset = frame(&recipe(4), 3, false);
    reset.reset = true;
    assert!(current.merge(reset).is_err());
    assert_eq!(current, saved);
    let mut conflict = frame(&recipe(4), 3, false);
    conflict.water_removed.push(7);
    assert!(current.merge(conflict).is_err());
    assert_eq!(current, saved);
    // Explicit full replacement may rewind or fork at the same checkpoint.
    current.merge(frame(&recipe(1), 0, false)).unwrap();
    assert_eq!(current.resident_water[0].recipe, recipe(1).patch(0));
    assert!(current.resident_water[0].checkpoint.is_some());
}
#[test]
fn slow_mailboxes_keep_only_unconsumed_steps_and_replace_at_checkpoint_changes() {
    let feed = amoris_assets::Feed::new();
    let mailbox = feed.subscribe();
    let initial = RenderFrame {
        reset: true,
        ..frame(&recipe(1), 0, true)
    };
    feed.publish(RenderFrame::default(), Some(initial));
    let received = mailbox.take();
    assert_eq!(received.resident_water[0].recipe.steps.len(), 1);
    for n in 2..=7 {
        feed.publish(frame(&recipe(n), n - 1, false), None);
    }
    let pending = mailbox.take();
    assert_eq!(pending.resident_water[0].recipe.start, 1);
    assert_eq!(pending.resident_water[0].recipe.steps.len(), 6);
    assert!(pending.resident_water[0].checkpoint.is_none());
    let restored = pending.resident_water[0]
        .recipe
        .apply(Some(&recipe(1)))
        .unwrap();
    assert_eq!(restored, recipe(7));
    feed.publish(frame(&recipe(8), 7, false), None);
    let mut c = seed();
    c.columns[0][1] = 3.0f32.to_bits();
    let next = FieldRecipe::new(c.key(), vec![], c.digest(), "test field".into());
    let mut change = frame(&next, 0, false);
    change.resident_water[0].checkpoint = Some(c.checked().unwrap().into());
    feed.publish(change, None);
    let pending = mailbox.take();
    assert_eq!(pending.resident_water[0].recipe, next.patch(0));
    assert!(pending.resident_water[0].checkpoint.is_some());
}

#[test]
fn shared_inputs_keep_cached_heads_and_the_complete_recipe_wire_format() {
    #[derive(serde::Serialize)]
    struct WireRecipe {
        checkpoint: amoris_sim::ContentHash,
        steps: Vec<FieldStep>,
        digest: [u8; 32],
        environment: String,
    }
    let old = recipe(3);
    let mut next = old.clone();
    assert!(std::sync::Arc::ptr_eq(&old.steps()[0], &next.steps()[0]));
    next.push_step((*recipe(4).steps()[3]).clone());
    next.digest = [4; 32];
    assert_eq!(old, recipe(3));
    assert_eq!(next, recipe(4));
    for n in 0..=4 {
        assert_eq!(next.prefix_key(n), Some(recipe(n).program_key()));
    }
    assert_eq!(next.prefix_key(5), None);
    let wire = WireRecipe {
        checkpoint: next.checkpoint(),
        steps: next.steps().iter().map(|s| (**s).clone()).collect(),
        digest: next.digest,
        environment: next.environment.clone(),
    };
    assert_eq!(
        serde_json::to_value(&next).unwrap(),
        serde_json::to_value(&wire).unwrap()
    );
    let config = bincode2::config::standard().with_fixed_int_encoding();
    let encoded = bincode2::serde::encode_to_vec(&next, config).unwrap();
    assert_eq!(
        encoded,
        bincode2::serde::encode_to_vec(&wire, config).unwrap()
    );
    let (decoded, used): (FieldRecipe, _) =
        bincode2::serde::decode_from_slice(&encoded, config).unwrap();
    assert_eq!(used, encoded.len());
    assert_eq!(decoded, next);
    assert_eq!(decoded.patch(3).apply(Some(&old)).unwrap(), next);
    let mut forged = serde_json::to_value(&next).unwrap();
    forged["heads"] = serde_json::json!([]);
    assert!(serde_json::from_value::<FieldRecipe>(forged).is_err());
    next.reset(seed().key(), seed().digest());
    assert_eq!(next, recipe(0));
    assert_eq!(old, recipe(3));
}

#[test]
fn checkpoint_ownership_is_shared_but_encoded_content_remains_value_based() {
    use std::sync::Arc;
    let r = recipe(0);
    let original = frame(&r, 0, true);
    let shared = original.resident_water[0]
        .checkpoint
        .as_ref()
        .unwrap()
        .clone();
    let feed = amoris_assets::Feed::new();
    let a = feed.subscribe();
    let b = feed.subscribe();
    feed.publish(original.clone(), Some(original.clone()));
    for received in [a.take(), b.take()] {
        assert!(
            Arc::ptr_eq(
                received.resident_water[0].checkpoint.as_ref().unwrap(),
                &shared
            ),
            "mailbox cloned checkpoint columns"
        );
    }
    // A slow subscriber retains its seed while suffixes coalesce and validation clones frames.
    feed.publish(original.clone(), Some(original.clone()));
    for n in 1..=8 {
        feed.publish(frame(&recipe(n), n - 1, false), None);
    }
    for received in [a.take(), b.take()] {
        assert_eq!(received.resident_water[0].recipe.steps.len(), 8);
        assert!(
            Arc::ptr_eq(
                received.resident_water[0].checkpoint.as_ref().unwrap(),
                &shared
            ),
            "coalescing deep-copied the retained seed"
        );
    }
    let decoded = RenderFrame::decode(&original.encode()).unwrap();
    assert_eq!(decoded, original);
    assert!(
        !Arc::ptr_eq(
            decoded.resident_water[0].checkpoint.as_ref().unwrap(),
            &shared
        ),
        "wire data is content, never an address"
    );
    #[derive(serde::Serialize)]
    struct Owned {
        id: u64,
        origin: [f32; 2],
        cell_m: f32,
        look: WaterLookView,
        checkpoint: Option<FieldCheckpoint>,
        recipe: amoris_assets::water_field::FieldPatch,
        edge_level: Option<f32>,
    }
    let update = &original.resident_water[0];
    let owned = Owned {
        id: update.id,
        origin: update.origin,
        cell_m: update.cell_m,
        look: update.look.clone(),
        checkpoint: Some(shared.as_raw().clone()),
        recipe: update.recipe.clone(),
        edge_level: update.edge_level,
    };
    assert_eq!(
        serde_json::to_value(update).unwrap(),
        serde_json::to_value(&owned).unwrap(),
        "the feed adapter changed the owned record's JSON contents"
    );
    let mut changed = shared.as_raw().clone();
    changed.columns[0][1] = 3.0f32.to_bits();
    let changed = Arc::new(changed.checked().unwrap());
    assert_eq!(shared.columns[0][1], 2.0f32.to_bits());
    assert!(!Arc::ptr_eq(&changed, &shared));
}
