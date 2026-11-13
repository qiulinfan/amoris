//! Layers (docs/spec/animation.md 3): the graphs made before additive layers, layer weights and
//! mask entries without descendants evaluate as they did, bit for bit (thornkeep's five graphs over
//! a fixed sequence of parameters and montages have the recorded digests).
#![cfg(feature = "import")]

use std::path::{Path, PathBuf};

use amoris_assets::anim::{AnimCommand, AnimParam, AnimState, ClipSet, GraphDef, Rig, graph};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A project's graph bound to the files it names.
fn rig(project: &Path, graph: &str) -> (Rig, Vec<ClipSet>) {
    let text = std::fs::read_to_string(project.join(graph)).unwrap();
    let def = GraphDef::parse(&serde_json::from_str(&text).unwrap()).unwrap();
    let mut sets: Vec<(String, ClipSet)> = Vec::new();
    for file in std::iter::once(&def.skeleton).chain(&def.libraries) {
        if !sets.iter().any(|(p, _)| p == file) {
            let set = amoris_assets::anim::load::load(&project.join(file)).unwrap();
            sets.push((file.clone(), set));
        }
    }
    let rig = Rig::new(def, &sets).unwrap();
    (rig, sets.into_iter().map(|s| s.1).collect())
}

fn mix(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn set(st: &mut AnimState, name: &str, value: f64) {
    match st.params.iter_mut().find(|p| p.name == name) {
        Some(p) => p.value = value,
        None => st.params.push(AnimParam {
            name: name.into(),
            value,
        }),
    }
}

fn play(st: &mut AnimState, montage: &str) {
    st.commands.push(AnimCommand {
        op: "play".into(),
        montage: montage.into(),
        rate: 1.0,
        ..AnimCommand::default()
    });
}

/// The digest of 600 ticks of a thornkeep graph: speeds through its blend space, a guard, montages
/// in the full and the masked upper slot (one interrupting another), a stop and a death; every
/// joint's, morph weight's and root motion's bits and every notify.
fn thornkeep(graph: &str) -> u64 {
    let (rig, sets) = rig(&repo().join("samples/thornkeep"), graph);
    let mut st = rig.initial(graph);
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for tick in 0..600u32 {
        if tick < 240 {
            set(&mut st, "speed", f64::from(tick) * 0.03);
        }
        match tick {
            60 => play(&mut st, "light_a"),
            150 => play(&mut st, "drink"),
            200 => play(&mut st, "roll"),
            215 => play(&mut st, "heavy"),
            260 => set(&mut st, "blocking", 1.0),
            330 => set(&mut st, "blocking", 0.0),
            360 => play(&mut st, "hit_chest"),
            380 => st.commands.push(AnimCommand {
                op: "stop".into(),
                fade: 0.15,
                ..AnimCommand::default()
            }),
            500 => set(&mut st, "dead", 1.0),
            _ => {}
        }
        let out = graph::step(&rig, &sets, &mut st, 1.0 / 60.0);
        for x in &out.pose {
            for v in x.t.iter().chain(&x.r).chain(&x.s) {
                mix(&mut h, &v.to_bits().to_le_bytes());
            }
        }
        for w in &out.morph {
            mix(&mut h, &w.to_bits().to_le_bytes());
        }
        for v in &out.root_motion {
            mix(&mut h, &v.to_bits().to_le_bytes());
        }
        for n in &out.notices {
            mix(&mut h, n.name.as_bytes());
            mix(&mut h, n.phase.as_bytes());
        }
    }
    h
}

#[test]
fn graphs_without_additive_layers_evaluate_as_before() {
    let got: Vec<(&str, u64)> = [
        "anim/archer.anim.json",
        "anim/brigand.anim.json",
        "anim/brute.anim.json",
        "anim/sellsword.anim.json",
        "anim/warden.anim.json",
    ]
    .into_iter()
    .map(|g| (g, thornkeep(g)))
    .collect();
    for (g, h) in &got {
        println!("{g}: {h:#018x}");
    }
    assert_eq!(got, THORNKEEP, "thornkeep's poses changed");
}

const THORNKEEP: [(&str, u64); 5] = [
    ("anim/archer.anim.json", 0xcbf9_b44b_805f_2b3b),
    ("anim/brigand.anim.json", 0x54f0_c950_9833_c64f),
    ("anim/brute.anim.json", 0x32da_0eeb_5f0e_28ac),
    ("anim/sellsword.anim.json", 0x1913_ccf4_b2f5_28ef),
    ("anim/warden.anim.json", 0x65d9_b346_df3e_bf4c),
];

/// The layers fixture's graph (`tests/fixtures/scenes/layers`, its `gen_layers.py` writes the
/// model), and the same graph with its additive layers and their montage left out.
fn fixture() -> ((Rig, Vec<ClipSet>), Rig) {
    let project = repo().join("tests/fixtures/scenes/layers");
    let (rig, sets) = rig(&project, "anim/figure.anim.json");
    let mut def = rig.def.clone();
    def.layers.retain(|l| l.additive.is_none());
    def.montages.clear();
    def.ik.clear();
    let named: Vec<(String, ClipSet)> = vec![(def.skeleton.clone(), sets[0].clone())];
    let plain = Rig::new(def, &named).unwrap();
    ((rig, sets), plain)
}

/// 300 ticks of a fixture's graph, driven as its script drives it (without the IK goal, which the
/// runtime applies after the layers).
fn layered(rig: &Rig, sets: &[ClipSet]) -> Vec<graph::Output> {
    let mut st = rig.initial("anim/figure.anim.json");
    (0..300u32)
        .map(|t| {
            if (30..=90).contains(&t) {
                set(&mut st, "lean", f64::from(t - 30) / 60.0);
            }
            if (200..=240).contains(&t) {
                set(&mut st, "lean", f64::from(240 - t) / 40.0);
            }
            match t {
                60 => set(&mut st, "speed", 1.2),
                150 => set(&mut st, "speed", 0.6),
                250 => set(&mut st, "speed", 0.0),
                170 => set(&mut st, "flinch", 0.5),
                110 | 130 | 180 => play(&mut st, "flinch"),
                _ => {}
            }
            graph::step(rig, sets, &mut st, 1.0 / 60.0)
        })
        .collect()
}

#[test]
fn a_layered_sequence_has_the_same_bits_everywhere() {
    let ((rig, sets), plain) = fixture();
    let out = layered(&rig, &sets);
    let base = layered(&plain, &sets);
    let chest = rig.skeleton.find("chest").unwrap();
    let head = rig.skeleton.find("head").unwrap();
    let forearm = rig.skeleton.find("forearm_r").unwrap();
    let same = |a: &graph::Output, b: &graph::Output| a.pose == b.pose && a.morph == b.morph;
    // Before the lean's weight rises, the additive layers add nothing: the bits are the plain
    // graph's (its override arm at half weight included).
    for t in 0..=30 {
        assert!(same(&out[t], &base[t]), "tick {t}");
    }
    assert_ne!(
        base[20].pose[forearm],
        rig.skeleton.rest()[forearm],
        "the arm waves"
    );
    // At full lean (its clip held at its end) the face smiles 0.6, and the chest leans forward.
    assert_eq!(out[100].morph, [0.6, 0.0]);
    assert!(out[100].pose[chest].r[0] > base[100].pose[chest].r[0] + 0.1);
    // The flinch blinks; at half its weight it blinks half as much.
    let blink = |o: &[graph::Output], from: usize| {
        o[from..from + 30]
            .iter()
            .map(|x| x.morph[1])
            .fold(0.0f32, f32::max)
    };
    assert!(
        (blink(&out, 130) - 1.0).abs() < 0.05,
        "{}",
        blink(&out, 130)
    );
    assert!(
        (blink(&out, 180) - 0.5).abs() < 0.03,
        "{}",
        blink(&out, 180)
    );
    assert!(
        out[190].pose[head] != base[190].pose[head],
        "the head drops"
    );
    // With the lean back to 0 and the last flinch faded out, the bits are the plain graph's again.
    for t in 241..300 {
        assert!(same(&out[t], &base[t]), "tick {t}");
    }
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for o in &out {
        for x in &o.pose {
            for v in x.t.iter().chain(&x.r).chain(&x.s) {
                mix(&mut h, &v.to_bits().to_le_bytes());
            }
        }
        for w in &o.morph {
            mix(&mut h, &w.to_bits().to_le_bytes());
        }
    }
    // The evaluator's adds, subtracts, multiplies, divisions and square roots in a fixed order
    // give these bits in debug and release builds and on every target.
    assert_eq!(h, LAYERED, "layered digest {h:#018x}");
}

const LAYERED: u64 = 0x74ee_9a04_a9c9_8d38;
