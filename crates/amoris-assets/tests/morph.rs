//! Morph targets (docs/spec/animation.md 5a) from `tests/fixtures/scenes/morph` (its
//! `gen_morph.py` writes `models/morph.gltf`): the renderer's import and the simulation's read the
//! same targets, names, default weights and weight channels; a graph blends weights from clips, a
//! fade and a masked slot with the recorded bits; a clip from another file binds by target name.
#![cfg(feature = "import")]

use std::path::{Path, PathBuf};

use amoris_assets::anim::clip::{Bound, Clip, ClipSet, Keys, Track};
use amoris_assets::anim::{AnimCommand, AnimParam, GraphDef, Rig, graph};
use amoris_assets::animation::{morph_offsets, morph_weights};
use amoris_assets::mesh::{Interpolation, MorphTargets};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/scenes/morph")
}

#[test]
fn the_renderers_import_keeps_targets_names_defaults_and_weight_channels() {
    let a = amoris_assets::import::import_gltf(&fixture().join("models/morph.gltf")).unwrap();
    let plate = a.meshes.iter().find(|m| m.name == "plate").unwrap();
    let body = a.meshes.iter().find(|m| m.name == "body").unwrap();
    let t = plate.morph.as_ref().expect("the plate's targets");
    assert_eq!(t.names, ["right", "lift"]);
    let n = plate.vertices.len();
    assert_eq!(n, 5, "a morphed mesh keeps its vertices");
    assert_eq!(t.deltas.len(), 2 * n * MorphTargets::STRIDE);
    const S: usize = MorphTargets::STRIDE;
    for (i, v) in plate.vertices.iter().enumerate() {
        // "right" moves every vertex along +x; "lift" raises the centre alone and tilts its normal.
        assert_eq!(&t.deltas[i * S..i * S + 3], &[0.6, 0.0, 0.0]);
        let lift = &t.deltas[(n + i) * S..(n + i) * S + S];
        if v.position == [0.0, 0.0, 0.0] {
            assert_eq!(lift, &[0.0, 0.3, 0.0, 0.0, 0.5, -0.5, 0.0, 0.0, 0.0]);
        } else {
            assert_eq!(lift, &[0.0; S]);
        }
    }
    assert!((t.reach(n) - 0.9).abs() < 1e-6, "{}", t.reach(n));
    assert_eq!(body.morph.as_ref().unwrap().names, ["right"]);
    assert!(body.skin.is_some());
    // The nodes' defaults, and which node each drawn mesh hangs from.
    let names: Vec<&str> = a.skeleton.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["root", "plate", "bone", "body"]);
    assert_eq!(a.skeleton[1].weights, [0.0, 0.0]);
    assert!(a.skeleton[2].weights.is_empty());
    assert_eq!(a.skeleton[3].weights, [0.0]);
    assert_eq!(morph_offsets(&a), [0, 0, 2, 2, 3]);
    let nodes: Vec<Option<usize>> = a.nodes.iter().map(|d| d.node).collect();
    assert_eq!(nodes, [Some(1), Some(3)]);
    // Weight channels: linear, step and cubic.
    let clip = |name: &str| a.animations.iter().find(|c| c.name == name).unwrap();
    let wave = clip("wave");
    assert_eq!(wave.morph.len(), 2);
    assert_eq!(wave.channels.len(), 1, "the turn stays a joint channel");
    assert_eq!(morph_weights(&a, Some(wave), 0.5, 1), [0.5, 0.25]);
    assert_eq!(morph_weights(&a, Some(wave), 1.5, 1), [0.5, 0.75]);
    assert_eq!(morph_weights(&a, Some(wave), 5.0, 3), [1.0]);
    assert_eq!(morph_weights(&a, None, 0.5, 1), [0.0, 0.0]);
    let blink = clip("blink");
    assert_eq!(blink.morph[0].interpolation, Interpolation::Step);
    assert_eq!(morph_weights(&a, Some(blink), 0.4, 1), [0.0, 0.0]);
    assert_eq!(morph_weights(&a, Some(blink), 0.6, 1), [0.0, 1.0]);
    let ease = clip("ease");
    assert_eq!(ease.morph[0].interpolation, Interpolation::Cubic);
    assert_eq!(ease.morph[0].values.len(), 12);
    // Flat tangents: the Hermite basis alone, 0.5 halfway.
    assert_eq!(morph_weights(&a, Some(ease), 0.5, 1), [0.5, 0.0]);
}

#[test]
fn the_simulations_read_keeps_the_same_targets_and_keys() {
    let set = amoris_assets::anim::load::load(&fixture().join("models/morph.gltf")).unwrap();
    let j = &set.skeleton.joints;
    assert_eq!(j[1].targets, ["right", "lift"]);
    assert_eq!(j[1].weights, [0.0, 0.0]);
    assert!(j[2].targets.is_empty());
    assert_eq!(j[3].targets, ["right"]);
    assert_eq!(set.skeleton.morph_offsets(), [0, 0, 2, 2, 3]);
    let clip = |name: &str| set.clips.iter().find(|c| c.name == name).unwrap();
    let plate = |c: &Clip| {
        c.tracks
            .iter()
            .find(|t| t.joint == "plate")
            .unwrap()
            .w
            .clone()
    };
    assert_eq!(plate(clip("wave")).values, [0.0, 0.0, 1.0, 0.5, 0.0, 1.0]);
    assert!(plate(clip("blink")).step);
    // A cubic channel keeps its values at the keys.
    assert_eq!(plate(clip("ease")).values, [0.0, 0.0, 1.0, 0.0]);
}

/// The fixture's graph bound to its file.
fn rig() -> (Rig, Vec<ClipSet>) {
    let set = amoris_assets::anim::load::load(&fixture().join("models/morph.gltf")).unwrap();
    let text = std::fs::read_to_string(fixture().join("anim/morph.anim.json")).unwrap();
    let def = GraphDef::parse(&serde_json::from_str(&text).unwrap()).unwrap();
    let sets = vec![("models/morph.gltf".to_owned(), set)];
    let rig = Rig::new(def, &sets).unwrap();
    (rig, sets.into_iter().map(|s| s.1).collect())
}

/// The weights of 200 ticks of the fixture's graph, driven as its script drives it (without the
/// override, which the runtime applies).
fn run() -> Vec<Vec<f32>> {
    let (rig, sets) = rig();
    assert_eq!(rig.morph_len(), 3);
    let names: Vec<&str> = rig.morph_targets().map(|t| t.0).collect();
    assert_eq!(names, ["right", "lift", "right"]);
    let mut st = rig.initial("anim/morph.anim.json");
    let mut out = Vec::new();
    for tick in 0..200u32 {
        let set = |st: &mut amoris_assets::anim::AnimState, v: f64| {
            st.params = vec![AnimParam {
                name: "still".into(),
                value: v,
            }];
        };
        match tick {
            70 => st.commands.push(AnimCommand {
                op: "play".into(),
                montage: "blink".into(),
                rate: 1.0,
                ..AnimCommand::default()
            }),
            100 => set(&mut st, 1.0),
            160 => set(&mut st, 0.0),
            _ => {}
        }
        out.push(graph::step(&rig, &sets, &mut st, 1.0 / 60.0).morph);
    }
    out
}

#[test]
fn a_graph_blends_weights_with_the_same_bits_everywhere() {
    let w = run();
    // The first tick samples the wave a tick in; its weights are the clip's.
    let at = |t: f32| [t, t * 0.5, t];
    assert_eq!(w[0], at((1.0f64 / 60.0) as f32));
    // Before the blink, the clip alone, half a second in (the layer's time sums the ticks).
    let half = (0..30).fold(0.0f64, |t, _| t + 1.0 / 60.0) as f32;
    assert_eq!(w[29], at(half));
    // While the blink holds its slot at full weight (past its 0.1 s fade, before its step at
    // 0.5 s), the face mask's half weight draws the plate's weights halfway to the blink's zeros,
    // and leaves the body's to the base layer.
    let (blink, base) = (&w[85], &run_base()[85]);
    assert_eq!(blink[0], base[0] * 0.5, "{blink:?} over {base:?}");
    assert_eq!(blink[1], base[1] * 0.5);
    assert_eq!(blink[2], base[2]);
    assert!(base[0] > 0.0 && base[1] > 0.0);
    // The digest of every weight's bits over the run (the evaluator's adds, subtracts and multiplies
    // in a fixed order give these bits on every target).
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for x in w.iter().flatten() {
        for b in x.to_bits().to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    assert_eq!(h, DIGEST, "weights digest {h:#018x}");
}

/// The run without the blink: the base layer's weights alone.
fn run_base() -> Vec<Vec<f32>> {
    let (rig, sets) = rig();
    let mut st = rig.initial("anim/morph.anim.json");
    (0..200u32)
        .map(|tick| {
            if tick == 100 || tick == 160 {
                st.params = vec![AnimParam {
                    name: "still".into(),
                    value: if tick == 100 { 1.0 } else { 0.0 },
                }];
            }
            graph::step(&rig, &sets, &mut st, 1.0 / 60.0).morph
        })
        .collect()
}

const DIGEST: u64 = 0x67d2_992f_f5fe_05cb;

#[test]
fn a_clip_from_another_file_binds_its_weights_by_target_name() {
    let (rig, sets) = rig();
    let target = &rig.skeleton;
    // Another file whose plate names its targets the other way round, and has one more.
    let mut source = sets[0].skeleton.clone();
    source.joints[1].targets = vec!["lift".into(), "frown".into(), "right".into()];
    source.joints[1].weights = vec![0.0; 3];
    let other = ClipSet {
        skeleton: source,
        clips: vec![Clip {
            name: "other".into(),
            duration: 1.0,
            tracks: vec![Track {
                joint: "plate".into(),
                t: Keys::default(),
                r: Keys::default(),
                s: Keys::default(),
                w: Keys {
                    times: vec![0.0],
                    values: vec![0.25, 0.5, 0.75],
                    step: false,
                },
            }],
        }],
    };
    let b = Bound::new(target, &other, 0, 0, &[], "pelvis");
    let mut w = target.rest_weights();
    b.sample_weights(std::slice::from_ref(&other), 0.0, &mut w);
    // "lift" takes 0.25, "right" 0.75, "frown" nothing; the body keeps its default.
    assert_eq!(w, [0.75, 0.25, 0.0]);
}
